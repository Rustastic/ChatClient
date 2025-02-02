use crate::{network_structure::NetworkTopology, packet_cache::PacketCache};
use assembler::Assembler;
use colored::Colorize;
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{error, info, warn};

use messages::{client_commands::*, high_level_messages::*};

use rand::Rng;
use std::collections::{HashMap, HashSet};
use wg_2024::{
    network::{NodeId, SourceRoutingHeader},
    packet::{
        self, Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet,
        PacketType,
    },
};


todo!("handling errori nell'invio di messaggi");
todo!("refactoring handling messaggi");
todo!("handle server type nella read message");



pub struct ChatClient {
    id: NodeId,
    running: bool,
    registered: Option<NodeId>,
    client_list: Vec<NodeId>,
    assembler: Assembler,
    network_knowledge: NetworkTopology,
    communication_server_list: Vec<NodeId>,
    message_buffer: Vec<Message>,

    controller_send: Sender<ChatClientEvent>,
    controller_recv: Receiver<ChatClientCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_id_received: HashSet<(u64, NodeId)>,
    packet_cache: PacketCache,

    flood_id_counter: u64,
    session_id_counter: u64,
}

impl ChatClient {
    //build the chat client
    pub fn new(
        id: NodeId,
        controller_send: Sender<ChatClientEvent>,
        controller_recv: Receiver<ChatClientCommand>,
        packet_recv: Receiver<Packet>,
        packet_send: HashMap<NodeId, Sender<Packet>>,
    ) -> Self {
        Self {
            id,
            assembler: Assembler::new(),
            network_knowledge: NetworkTopology::new(),
            client_list: Vec::new(),
            message_buffer: Vec::new(),
            controller_send,
            controller_recv,
            packet_recv,
            packet_send,
            flood_id_received: HashSet::new(),
            packet_cache: PacketCache::new(),
            flood_id_counter: 0,
            session_id_counter: 0,
            running: false,
            registered: None,
            communication_server_list: Vec::new(),
        }
    }
    pub fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        self.handle_command(command);
                    }
                },

                recv(self.packet_recv) -> packet => {
                    if let Ok(packet) = packet {
                        self.handle_packet(packet);
                    }
                },

            }
        }
    }

    fn handle_command(&mut self, command: ChatClientCommand) {
        match command {
            ChatClientCommand::AddSender(node_id, sender) => {
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.packet_send.entry(node_id)
                {
                    info!(
                        "{} Adding sender: {} to [ ChatClient {} ]",
                        "✓".green(),
                        node_id,
                        self.id,
                    );
                    e.insert(sender);
                } else {
                    warn!(
                        "{} [ ChatClient {} ] is already connected to [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        node_id
                    );
                }
            }
            ChatClientCommand::RemoveSender(node_id) => {
                if self.packet_send.contains_key(&node_id) {
                    info!(
                        "{} Removing sender: {} from [ ChatClient {} ]",
                        "✓".green(),
                        node_id,
                        self.id
                    );
                    self.packet_send.remove(&node_id);
                } else {
                    warn!(
                        "{} [ ChatClient {} ] is already disconnected from [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        node_id
                    );
                }
            }
            ChatClientCommand::InitFlooding => {
                info!(
                    "{} [ ChatClient {} ]: Initiating flooding process",
                    "ℹ".blue(),
                    self.id
                );
                self.start_flooding();
            }
            ChatClientCommand::StartChatClient => {
                self.running = true;
                info!(
                    "{} [ ChatClient {} ]: Starting ChatClient",
                    "ℹ".blue(),
                    self.id
                );
                self.query_communication_servers();
            }
            ChatClientCommand::SendMessageTo(destination_id, text) => {
                let session_id = self.session_id_counter;
                self.session_id_counter += 1;

                if let Some(server_id) = self.registered {
                    let message = Message::new_client_message(
                        session_id,
                        self.id,
                        server_id,
                        ClientMessage::SendMessage {
                            recipient_id: destination_id,
                            content: text,
                        },
                    );

                    self.send_message_to(message);
                } else {
                    error!(
                        "{} [ ChatClient {} ]: Cannot send message, not registered to any server",
                        "✗".red(),
                        self.id
                    );
                    self.controller_send
                        .send(ChatClientEvent::ErrorNotRegistered)
                        .unwrap();
                }
            }
            ChatClientCommand::RegisterTo(server_id) => self.register_to(server_id),
            ChatClientCommand::GetClientList => self.get_client_list(),
            ChatClientCommand::LogOut => self.logout(),
        }
    }

    fn handle_packet(&mut self, mut packet: Packet) {
        if let PacketType::FloodRequest(flood_request) = packet.clone().pack_type {
            let flood_id = flood_request.flood_id;
            let flood_initiator = flood_request.initiator_id;
            self.handle_flood_request(flood_request, &packet);
            self.flood_id_received.insert((flood_id, flood_initiator));
        } else if self.check_packet_correct_id(packet.clone()) {
            // Increase hop_index
            packet.routing_header.increase_hop_index();

            // If the destination has been reached, and it is a Drone (invalid destination)
            if packet.routing_header.hop_index == packet.routing_header.hops.len() {
                // the client received a packet
                match packet.clone().pack_type {
                    PacketType::MsgFragment(fragment) => self.process_fragment(fragment, &packet),
                    PacketType::Ack(ack) => self.process_ack(ack, &packet),
                    PacketType::Nack(nack) => self.process_nack(nack, &packet),
                    PacketType::FloodResponse(flood_response) => {
                        self.process_flood_response(flood_response)
                    }
                    _ => unreachable!(),
                }
            } else {
                // Check if the next hop is a valid neighbor
                if !self.check_neighbor(&packet) {
                    //Step4
                    let neighbor = packet.routing_header.hops[packet.routing_header.hop_index];
                    error!(
                    "{} [ Drone {} ]: can't send packet to Drone {} because it is not its neighbor",
                    "✗".red(),
                    self.id,
                    neighbor
                );
                    self.send_nack(packet, None, NackType::ErrorInRouting(self.id));
                    return;
                }

                info!(
                    "{} Packet with [ session_id: {} ] is being handled from [ Drone {} ]",
                    "✓".green(),
                    packet.session_id,
                    self.id
                );

                // Handle packet types: Nack, Ack, MsgFragment, FloodResponse
                match packet.clone().pack_type {
                    PacketType::Nack(_nack) => self.handle_ack_nack(packet),
                    PacketType::Ack(_ack) => self.handle_ack_nack(packet),
                    PacketType::MsgFragment(fragment) => self.handle_fragment(packet, fragment),
                    PacketType::FloodRequest(_) => unreachable!(),
                    PacketType::FloodResponse(flood_response) => {
                        self.handle_flood_response(&flood_response, &packet);
                    }
                }
            }
        }
    }

    fn check_packet_correct_id(&self, packet: Packet) -> bool {
        if self.id == packet.routing_header.hops[packet.routing_header.hop_index] {
            true
        } else {
            self.send_nack(packet, None, NackType::UnexpectedRecipient(self.id));
            error!(
                "{} [ Drone {} ]: does not correspond to the Drone indicated by the `hop_index`",
                "✗".red(),
                self.id
            );
            false
        }
    }

    fn check_neighbor(&self, packet: &Packet) -> bool {
        let destination = packet.routing_header.hops[packet.routing_header.hop_index];
        self.packet_send.contains_key(&destination)
    }

    fn handle_fragment(&mut self, mut packet: Packet, fragment: Fragment) {
        // Add the fragment to the buffer
        info!(
                "{} [ ChatClient {} ]: forwarded the the fragment [ fragment_index: {} ] of the Packet [ session_id: {} ]",
                "✓".green(),
                self.id,
                fragment.fragment_index,
                packet.session_id
            );

        self.forward_packet(packet);
    }
    // ho cambiato il nome per la send_message del drone per evitare
    // ambiguita visto che ci sono anche i messaggi veri e propri
    fn forward_packet(&self, packet: Packet) -> bool {
        let destination = packet.routing_header.hops[packet.routing_header.hop_index];
        let packet_type = packet.pack_type.clone();

        // Try sending to the destination drone
        if let Some(sender) = self.packet_send.get(&destination) {
            match sender.send(packet.clone()) {
                Ok(()) => {
                    info!(
                        "{} [ ChatClient {} ]: was sent a {} packet to [ Node {} ]",
                        "✓".green(),
                        self.id,
                        packet_type,
                        destination
                    );
                    true
                }
                Err(e) => {
                    // In case of an error, forward the packet to the simulation controller
                    error!(
                        "{} [ ChatClient {} ]: Failed to send the {} to [ Node {} ]: {}",
                        "✗".red(),
                        self.id,
                        packet_type,
                        destination,
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    self.controller_send
                        .send(DroneEvent::PacketDropped(packet))
                        .unwrap();

                    warn!(
                        "└─>{} [ ChatClient {} ]: {} sent to Simulation Controller",
                        "!!!".yellow(),
                        self.id,
                        packet_type,
                    );

                    false
                }
            }
        } else {
            // Handle case where there is no connection to the destination drone
            if let PacketType::MsgFragment(_) = packet_type {
                error!(
                    "{} [ ChatClient {} ]: does not exist in the path",
                    "✗".red(),
                    destination
                );
            } else {
                error!(
                    "{} [ ChatClient {} ]: Failed to send the {}: No connection to [ Node {} ]",
                    "✗".red(),
                    self.id,
                    packet_type,
                    destination
                );

                warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                self.controller_send
                    .send(DroneEvent::PacketDropped(packet))
                    .unwrap();

                warn!(
                    "└─>{} [ Drone {} ]: {} sent to Simulation Controller",
                    "!!!".yellow(),
                    self.id,
                    packet_type,
                );
            }

            false
        }
    }

    fn handle_ack_nack(&mut self, mut packet: Packet) {
        if let PacketType::Nack(nack) = packet.clone().pack_type {
            warn!(
                "{} [ ChatClient {} ]: received a {}",
                "!!!".yellow(),
                self.id,
                packet.pack_type,
            );
            // Send a nack to the previous node
            self.send_nack(packet, None, NackType::Dropped);
        } else {
            warn!(
                "{} [ ChatClient {} ]: received a {}",
                "!!!".yellow(),
                self.id,
                packet.pack_type,
            );
            self.forward_packet(packet);
        }
    }

    fn send_nack(&self, mut packet: Packet, fragment: Option<Fragment>, nack_type: NackType) {
        // Reverse the routing header to get the previous hop
        packet.routing_header.reverse();
        let prev_hop = packet.routing_header.next_hop().unwrap();

        // Attempt to send the NACK to the previous hop
        if let Some(sender) = self.packet_send.get(&prev_hop) {
            let mut nack = Nack {
                fragment_index: 0, // Default fragment index for non-fragmented NACKs
                nack_type,
            };

            // If it's a fragment, set the fragment index and NACK type accordingly
            if let Some(frag) = fragment {
                nack = Nack {
                    fragment_index: frag.fragment_index,
                    nack_type: NackType::Dropped,
                };
            }

            // Update the packet type to NACK
            packet.pack_type = PacketType::Nack(nack);

            // Send the NACK to the previous hop
            match sender.send(packet.clone()) {
                Ok(()) => {
                    warn!(
                        "{} Nack was sent from [ ChatClient {} ] to [ Node {} ]",
                        "!!!".yellow(),
                        self.id,
                        prev_hop
                    );
                }
                Err(e) => {
                    // Handle failure to send the NACK, send to the simulation controller instead
                    warn!(
                        "{} [ ChatClient {} ]: Failed to send the Nack to [ Node {} ]: {}",
                        "✗".red(),
                        self.id,
                        prev_hop,
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    //there is an error in sending the packet, the drone should send the packet to the simulation controller
                    self.controller_send
                        .send(DroneEvent::PacketDropped(packet))
                        .unwrap();
                    warn!(
                        "└─>{} [ ChatClient {} ]: sent A Nack to the Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            // If no connection to the previous hop, send the NACK to the simulation controller
            error!(
                "{} [ ChatClient {} ]: Failed to send the Nack: No connection to {}",
                "✗".red(),
                self.id,
                prev_hop
            );

            warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

            // Create the NACK (same logic as above)
            let mut nack = Nack {
                fragment_index: 0,
                nack_type,
            };

            if let Some(frag) = fragment {
                nack = Nack {
                    fragment_index: frag.fragment_index,
                    nack_type: NackType::Dropped,
                };
            }
            packet.pack_type = PacketType::Nack(nack);

            // Send to the simulation controller
            self.controller_send
                .send(DroneEvent::PacketDropped(packet))
                .unwrap();
            warn!(
                "└─>{} [ ChatClient {} ]: sent A Nack to the Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    // ChatClient handles floodrequests and floodresponses in the same way the drone does

    fn handle_flood_request(&self, mut flood_request: FloodRequest, packet: &Packet) {
        // Determine the previous node that sent the packet
        let prev_node = if let Some(node) = flood_request.path_trace.last() {
            node.0
        } else {
            error!("This ChatClient is trying to handle a floodrequest started by itself");
            return;
        };

        // Add the current drone to the path-trace
        flood_request.path_trace.push((self.id, NodeType::Client));

        // Check if the flood request has already been processed
        if self
            .flood_id_received
            .contains(&(flood_request.flood_id, flood_request.initiator_id))
        {
            // If it has been processed, send a FloodResponse to the previous node

            let mut new_hops: Vec<u8> = flood_request
                .clone()
                .path_trace
                .into_iter()
                .map(|(id, _ntype)| id)
                .collect();

            new_hops.reverse();

            let new_routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: new_hops,
            };

            // Send the FloodResponse to the previous node
            self.send_flood_response(
                prev_node,
                &flood_request,
                new_routing_header,
                packet.session_id,
                format!(
                    "{} [ Drone {} ]: has already received a FloodRequest with flood_id: {}",
                    "!!!".yellow(),
                    self.id,
                    flood_request.flood_id
                )
                .as_str(),
            );
        } else if self.packet_send.len() == 1 {
            // If the drone has no neighbors except the previous node
            let mut new_hops: Vec<u8> = flood_request
                .clone()
                .path_trace
                .into_iter()
                .map(|(id, _ntype)| id)
                .collect();

            new_hops.reverse();

            let new_routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: new_hops,
            };

            // Send a FloodResponse indicating no further neighbors to forward the request
            self.send_flood_response(
                prev_node,
                &flood_request,
                new_routing_header,
                packet.session_id,
                format!(
                    "{} [ ChatClient {} ]: doesn't have any other neighbors to send the FloodRequest with flood_id: {}",
                    "!!!".yellow(),
                    self.id,
                    flood_request.flood_id
                )
                    .as_str(),
            );
        } else {
            // Forward the FloodRequest to all neighbors except the previous node
            for neighbor in self
                .packet_send
                .iter()
                .filter(|neighbor| *neighbor.0 != prev_node)
            {
                self.send_flood_request(
                    neighbor,
                    &flood_request,
                    packet.routing_header.clone(),
                    packet.session_id,
                );

                info!(
                    "{} [ ChatClient {} ]: sent a FloodRequest with flood_id: {} to the [ Node {} ]",
                    "✓".green(),
                    self.id,
                    flood_request.flood_id,
                    neighbor.0
                );
            }
        }
    }

    fn send_flood_request(
        &self,
        dest_node: (&NodeId, &Sender<Packet>),
        flood_request: &FloodRequest,
        routing_header: SourceRoutingHeader,
        session_id: u64,
    ) {
        let flood_id = flood_request.flood_id;
        let new_flood_request = FloodRequest {
            flood_id,
            initiator_id: flood_request.initiator_id,
            path_trace: flood_request.path_trace.clone(),
        };

        let new_packet = Packet {
            pack_type: PacketType::FloodRequest(new_flood_request),
            routing_header,
            session_id,
        };

        match dest_node.1.send(new_packet) {
            Ok(()) => info!(
                "{} [ ChatClient {} ]: sent the FloodRequest with flood_id: {} sent to [ Node {} ]",
                "✓".green(),
                self.id,
                flood_id,
                dest_node.0
            ),
            Err(e) => error!(
                "{} [ ChatClient {} ]: failed to send FloodRequest to the [ Node {} ]: {}",
                "✗".red(),
                self.id,
                dest_node.0,
                e
            ),
        };
    }

    fn handle_flood_response(&self, flood_response: &FloodResponse, packet: &Packet) {
        let new_routing_header = packet.routing_header.clone();

        // Prepare a new packet to send the flood response back
        let new_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response.clone()),
            routing_header: new_routing_header.clone(),
            session_id: packet.session_id,
        };

        // Try to send the FloodResponse to the next hop in the routing path
        if let Some(sender) = self
            .packet_send
            .get(&new_routing_header.hops[new_routing_header.hop_index])
        {
            match sender.send(new_packet.clone()) {
                Ok(()) => info!(
                    "{} [ ChatClient {} ]: sent a FloodResponse with flood_id: {} to [ Node {} ]",
                    "✓".green(),
                    self.id,
                    flood_response.flood_id,
                    &new_routing_header.hops[new_routing_header.hop_index]
                ),
                Err(e) => {
                    error!(
                        "{} [ ChatClient {} ]: failed to send the FloodResponse to the Node {}: {}",
                        "✗".red(),
                        self.id,
                        &new_routing_header.hops[new_routing_header.hop_index],
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();

                    warn!(
                        "└─>{} [ ChatClient {} ]: sent the FloodResponse to the Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            // If the next hop is unavailable, send the packet to the simulation controller
            error!(
                "{} [ ChatClient {} ]: failed to send the FloodResponse: No connection to [ Node {} ]",
                "✗".red(),
                self.id,
                &new_routing_header.hops[new_routing_header.hop_index]
            );

            warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

            // Send the packet to the simulation controller
            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();

            warn!(
                "└─>{} [ ChatClient {} ]: sent the FloodResponse to the Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    fn send_flood_response(
        &self,
        dest_node: NodeId,
        flood_request: &FloodRequest,
        routing_header: SourceRoutingHeader,
        session_id: u64,
        reason: &str,
    ) {
        let flood_response = FloodResponse {
            flood_id: flood_request.flood_id,
            path_trace: flood_request.path_trace.clone(),
        };

        let new_packet = Packet {
            pack_type: PacketType::FloodResponse(flood_response.clone()),
            routing_header,
            session_id,
        };

        if let Some(sender) = self.packet_send.get(&dest_node) {
            match sender.send(new_packet.clone()) {
                Ok(()) => info!(
                    "{} [ ChatClient {} ]: sent the FloodResponse to [ Node {} ]\n└─>Reason: {}",
                    "✓".green(),
                    self.id,
                    dest_node,
                    reason
                ),
                Err(e) => {
                    error!(
                        "{} [ ChatClient {} ]: Failed to send the FloodResponse to [ Node {} ]: {}",
                        "✗".red(),
                        self.id,
                        dest_node,
                        e
                    );

                    warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

                    self.controller_send
                        .send(DroneEvent::PacketDropped(new_packet))
                        .unwrap();

                    warn!(
                        "└─>{} [ ChatClient {} ]: FloodResponse sent to Simulation Controller",
                        "!!!".yellow(),
                        self.id
                    );
                }
            }
        } else {
            // Handle the case where there is no connection to the destination drone
            error!(
                "{} [ ChatClient {} ]: Failed to send the FloodResponse: No connection to [ Node {} ]",
                "✗".red(),
                self.id,
                dest_node
            );

            warn!("├─>{} Sending to Simulation Controller...", "!!!".yellow());

            self.controller_send
                .send(DroneEvent::PacketDropped(new_packet))
                .unwrap();

            warn!(
                "└─>{} [ ChatClient {} ]: FloodResponse sent to Simulation Controller",
                "!!!".yellow(),
                self.id
            );
        }
    }

    // the sim controller could send a command to the client that makes it start the flooding

    fn start_flooding(&mut self) {
        let flood_request = FloodRequest {
            flood_id: self.flood_id_counter, // da cambiare
            initiator_id: self.id,
            path_trace: vec![(self.id, NodeType::Client)],
        };

        self.flood_id_counter += 1;
        self.session_id_counter += 1;

        let routing_header = SourceRoutingHeader {
            hops: vec![],
            hop_index: 0,
        };

        for neighbor in self.packet_send.iter() {
            self.send_flood_request(
                neighbor,
                &flood_request,
                routing_header.clone(),
                self.session_id_counter,
            );
        }
    }

    fn process_flood_response(&mut self, flood_response: FloodResponse) {
        //in questo modo viene memorizzato il grafo
        self.network_knowledge
            .process_path_trace(flood_response.path_trace.clone());
        //missing logging
        info!(
            "{} [ ChatClient {} ]: Processed FloodResponse with flood_id: {}",
            "✓".green(),
            self.id,
            flood_response.flood_id
        );
    }

    fn process_fragment(&mut self, fragment: Fragment, packet: &Packet) {
        let new_routing_header = packet.routing_header.get_reversed();

        let ack_packet = Packet {
            routing_header: new_routing_header,
            session_id: self.session_id_counter,
            pack_type: PacketType::Ack(Ack {
                fragment_index: fragment.fragment_index,
            }),
        };

        self.forward_packet(ack_packet);

        let source_id = packet.routing_header.source().unwrap();

        if let Some(message) =
            self.assembler
                .process_fragment(fragment, packet.session_id, source_id)
        {
            //utilizzo un buffer perchè potrebbero arrivari più messaggi in contemporanea e alcuni potrebbero essere
            //persi
            self.message_buffer.push(message);
            self.read_message();
        }
    }

    fn process_ack(&mut self, ack: Ack, packet: &Packet) {
        info!(
            "{} [ ChatClient {} ]: Processed Ack for fragment_index: {}",
            "✓".green(),
            self.id,
            ack.fragment_index
        );

        if self
            .packet_cache
            .remove(packet.session_id, ack.fragment_index)
        {
            info!(
                "{} [ ChatClient {} ]: Removed packet with session_id: {} and fragment_index: {} from the cache",
                "✓".green(),
                self.id,
                packet.session_id,
                ack.fragment_index
            );
        } else {
            error!(
                "{} [ ChatClient {} ]: Failed to remove packet with session_id: {} and fragment_index: {} from the cache",
                "✗".red(),
                self.id,
                packet.session_id,
                ack.fragment_index
            );
        }
    }

    fn process_nack(&mut self, nack: Nack, packet: &Packet) {
        match nack.clone().nack_type {
            NackType::ErrorInRouting(unreachable_node) => {
                error!(
                    "{} [ ChatClient {} ]: Received a Nack indicating an error in the routing",
                    "✗".red(),
                    self.id
                );
                if let Some((incorrect_packet, _)) = self
                    .packet_cache
                    .remove_and_get(packet.session_id, nack.fragment_index)
                {
                    // sto assumendo che la destinazione è per forza giusta e che il problema occorre quando un drone viene fatto crasharee

                    let dest = incorrect_packet.routing_header.destination().unwrap();
                    let paths_to_dest = self.network_knowledge.find_paths_to(self.id, dest);
                    //al primo vettore contenuto in paths_to_dest non contenente unreachable_node si blocca l'iterazione e lo si seleziona come routing header

                    for path in paths_to_dest.iter() {
                        if !path.contains(&unreachable_node) {
                            let new_routing_header = SourceRoutingHeader {
                                hop_index: 0,
                                hops: path.clone(),
                            };

                            let new_packet = Packet {
                                pack_type: incorrect_packet.pack_type,
                                routing_header: new_routing_header,
                                session_id: incorrect_packet.session_id,
                            };

                            self.packet_cache.insert(new_packet.clone());

                            //log also the fragment_index
                            info!(
                                "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                                "✓".green(),
                                self.id,
                                new_packet.session_id,
                                nack.fragment_index,
                                dest
                            );

                            self.forward_packet(new_packet);
                            return;
                        }
                    }

                    self.network_knowledge.clear();

                    self.start_flooding();

                    let new_paths = self.network_knowledge.find_paths_to(self.id, dest);

                    let new_routing_header = SourceRoutingHeader {
                        hop_index: 0,
                        hops: new_paths[0].clone(),
                    };

                    let new_packet = Packet {
                        pack_type: incorrect_packet.pack_type,
                        routing_header: new_routing_header,
                        session_id: incorrect_packet.session_id,
                    };

                    self.packet_cache.insert(new_packet.clone());

                    info!(
                        "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                        "✓".green(),
                        self.id,
                        new_packet.session_id,
                        nack.fragment_index,
                        dest
                    );

                    self.forward_packet(new_packet);
                }
            } //identify the specific packet by (session,source,frag index)
            NackType::DestinationIsDrone => {
                // se la destinazione è un drone non sono in grado di risalire al vero destinatario è quindi impossibile inviare il messaggio
                // non dovrebbe accadere in ogni caso
                error!(
                    "{} [ ChatClient {} ]: Received a Nack indicating that the destination is a drone",
                    "✗".red(),
                    self.id
                );
            }
            NackType::Dropped => {
                error!(
                    "{} [ ChatClient {} ]: Received a Nack indicating that the packet was dropped",
                    "✗".red(),
                    self.id
                );

                if let Some((dropped_packet, drop_counter)) = self
                    .packet_cache
                    .remove_and_get(packet.session_id, nack.fragment_index)
                {
                    if drop_counter >= 3 {
                        error!(
                            "{} [ ChatClient {} ]: Packet with session_id: {} and fragment_index: {} has been dropped more than 3 times",
                            "✗".red(),
                            self.id,
                            dropped_packet.session_id,
                            nack.fragment_index
                        );

                        let mut packet_to_resend = dropped_packet.clone();

                        let dest = packet_to_resend.routing_header.destination().unwrap();

                        let paths_to_dest = self.network_knowledge.find_paths_to(self.id, dest);

                        let random_index = rand::thread_rng().gen_range(0..paths_to_dest.len());

                        let alt_path = paths_to_dest.get(random_index);

                        let new_routing_header = SourceRoutingHeader {
                            hop_index: 0,
                            hops: alt_path.unwrap().clone(),
                        };

                        packet_to_resend.routing_header = new_routing_header;

                        self.packet_cache.insert(packet_to_resend.clone());

                        info!(
                            "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                            "✓".green(),
                            self.id,
                            packet_to_resend.session_id,
                            nack.fragment_index,
                            dest
                        );

                        self.forward_packet(packet_to_resend);
                    } else {
                        let packet_to_resend = dropped_packet.clone();

                        self.packet_cache.insert(packet_to_resend.clone());

                        info!(
                            "{} [ ChatClient {} ]: Resending packet with session_id: {} and fragment_index: {}",
                            "✓".green(),
                            self.id,
                            packet_to_resend.session_id,
                            nack.fragment_index
                        );

                        self.forward_packet(packet_to_resend);
                    }
                }
            }
            NackType::UnexpectedRecipient(problematic_node) => {
                error!(
                    "{} [ ChatClient {} ]: Received a Nack indicating that the recipient was unexpected",
                    "✗".red(),
                    self.id
                );

                if let Some((incorrect_packet, _)) = self
                    .packet_cache
                    .remove_and_get(packet.session_id, nack.fragment_index)
                {
                    // sto assumendo che la destinazione è per forza giusta e che il problema occorre quando un drone viene fatto crasharee

                    let dest = incorrect_packet.routing_header.destination().unwrap();
                    let paths_to_dest = self.network_knowledge.find_paths_to(self.id, dest);
                    //al primo vettore contenuto in paths_to_dest non contenente unreachable_node si blocca l'iterazione e lo si seleziona come routing header

                    for path in paths_to_dest.iter() {
                        if !path.contains(&problematic_node) {
                            let new_routing_header = SourceRoutingHeader {
                                hop_index: 0,
                                hops: path.clone(),
                            };

                            let new_packet = Packet {
                                pack_type: incorrect_packet.pack_type,
                                routing_header: new_routing_header,
                                session_id: incorrect_packet.session_id,
                            };

                            self.packet_cache.insert(new_packet.clone());

                            //log also the fragment_index
                            info!(
                                "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                                "✓".green(),
                                self.id,
                                new_packet.session_id,
                                nack.fragment_index,
                                dest
                            );

                            self.forward_packet(new_packet);
                            return;
                        }
                    }

                    self.network_knowledge.clear();

                    self.start_flooding();

                    let new_paths = self.network_knowledge.find_paths_to(self.id, dest);

                    let new_routing_header = SourceRoutingHeader {
                        hop_index: 0,
                        hops: new_paths[0].clone(),
                    };

                    let new_packet = Packet {
                        pack_type: incorrect_packet.pack_type,
                        routing_header: new_routing_header,
                        session_id: incorrect_packet.session_id,
                    };

                    self.packet_cache.insert(new_packet.clone());

                    info!(
                        "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                        "✓".green(),
                        self.id,
                        new_packet.session_id,
                        nack.fragment_index,
                        dest
                    );

                    self.forward_packet(new_packet);
                }

                // this the case where a drone receives a packet and hops[hop_index] is not equal to the drone id
            }
        }
    }

    fn read_message(&mut self) {
        if let Some(message) = self.message_buffer.pop() {
            if message.destination_id != self.id {
                //destinazione sbagliata
                error!(
                    "{} [ ChatClient {} ]: Received a message with incorrect destination ID: {}",
                    "✗".red(),
                    self.id,
                    message.destination_id
                );
                return;
            }
            if let MessageContent::FromServer(server_message) = message.content {
                match server_message {
                    ServerMessage::ServerType(server_type) => {
                        if let ServerType::Chat = server_type {
                            self.communication_server_list.push(message.source_id);
                            info!(
                                "{} [ ChatClient {} ]: Discovered communication server [ CommunicationServer {} ]",
                                "✓".green(),
                                self.id,
                                message.source_id
                            );
                        }
                    }
                    ServerMessage::ClientList(client_list) => {
                        self.client_list = client_list;

                        info!(
                            "{} [ ChatClient {} ]: Updated client list: {:?}",
                            "ℹ".blue(),
                            self.id,
                            self.client_list
                        );

                        self.controller_send
                            .send(ChatClientEvent::ClientList(self.client_list.clone()))
                            .unwrap();
                    }
                    ServerMessage::MessageReceived { sender_id, content } => {
                        info!(
                            "{} [ ChatClient {} ]: Message received from [ Client {} ]: {}",
                            "✓".green(),
                            self.id,
                            sender_id,
                            content
                        );

                        self.controller_send
                            .send(ChatClientEvent::MessageReceived(sender_id, content))
                            .unwrap();
                    }
                    ServerMessage::UnreachableClient(client_id) => {
                        info!(
                            "{} [ ChatClient {} ]: Client {} is unreachable",
                            "!!!".yellow(),
                            self.id,
                            client_id
                        );

                        self.client_list.retain(|&id| id != client_id);

                        self.controller_send
                            .send(ChatClientEvent::Unreachable(client_id))
                            .unwrap();
                    }
                    ServerMessage::SuccessfulRegistration => {
                        self.registered = Some(message.source_id);
                        info!(
                            "{} [ ChatClient {} ]: Successfully registered to the server [ CommunicationServer {} ]",
                            "✓".green(),
                            self.id,
                            message.source_id
                        );
                        self.controller_send
                            .send(ChatClientEvent::SuccessfulRegistration(message.source_id))
                            .unwrap();
                    }
                    ServerMessage::SuccessfullLogOut => {
                        self.registered = None;
                        info!(
                            "{} [ ChatClient {} ]: Successfully logged out from the server [ CommunicationServer {} ]",
                            "✓".green(),
                            self.id,
                            message.source_id
                        );
                        self.controller_send
                            .send(ChatClientEvent::SuccessfulLogOut)
                            .unwrap();
                    }
                    _ => {
                        error!(
                            "{} [ ChatClient {} ]: Received a message intended for a web browser",
                            "✗".red(),
                            self.id
                        );
                    }
                }
            } else {
                // errore un client non può comunicare direttamente con un altro client
                error!(
                    "{} [ ChatClient {} ]: Received a message from an unexpected source: [ Client {} ]",
                    "✗".red(),
                    self.id,
                    message.source_id
                );
            }
        } else {
            info!(
                "{} [ ChatClient {} ]: No messages to read",
                "ℹ".blue(),
                self.id
            );
        }
    }
    todo!("gestire i casi di errore");
    fn send_message_to(&self, message: Message) {
        // controllo che il chat client conosca i server, che sia registrato ad uno di loro e che la destinazione sia registrata a quel server
        if self.running {
            self.get_client_list();
            if self.client_list.contains(&message.source_id) {
                let path = self
                    .network_knowledge
                    .find_paths_to(self.id, message.destination_id)[0];

                let routing_header = SourceRoutingHeader {
                    hop_index: 1,
                    hops: path.clone(),
                };

                if let Ok(fragments) = self.assembler.fragment_message(&message) {
                    for frag in fragments {
                        let packet_to_send = Packet {
                            routing_header: routing_header.clone(),
                            session_id: message.session_id,
                            pack_type: PacketType::MsgFragment(frag),
                        };

                        self.packet_cache.insert(packet_to_send.clone());

                        info!(
                            "{} [ ChatClient {} ]: Sending fragment with session_id: {} and fragment_index: {} to [ CommunicationServer {} ]",
                            "✓".green(),
                            self.id,
                            message.session_id,
                            frag.fragment_index,
                            message.destination_id
                        );

                        self.forward_packet(packet_to_send);
                    }
                }
            } else {
                // unreachable
            }
        } else {
            //error not runnig
        }
    }

    fn query_communication_servers(&mut self) {
        for server_id in self.network_knowledge.server_list.iter() {
            let session_id = self.session_id_counter;
            self.session_id_counter += 1;

            let query = Message::new_client_message(
                session_id,
                self.id,
                server_id,
                ClientMessage::GetServerType,
            );

            let path = self.network_knowledge.find_paths_to(self.id, server_id)[0];

            let routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: path,
            };

            if let Ok(fragments) = self.assembler.fragment_message(&query) {
                for frag in fragments {
                    let packet = Packet {
                        routing_header: routing_header.clone(),
                        session_id,
                        pack_type: PacketType::MsgFragment(frag),
                    };

                    self.packet_cache.insert(packet.clone());

                    info!(
                        "{} [ ChatClient {} ]: Querying server [ Server {} ] with session_id: {}",
                        "ℹ".blue(),
                        self.id,
                        server_id,
                        session_id
                    );

                    self.forward_packet(packet);
                }
            }
        }
    }

    fn get_client_list(&mut self) {
        if self.running
            && let Some(server_id) = self.registered
        {
            let session_id = self.session_id_counter;
            self.session_id_counter += 1;

            let message = Message::new_client_message(
                session_id,
                self.id,
                server_id,
                ClientMessage::GetClientList,
            );

            let path = self.network_knowledge.find_paths_to(self.id, server_id)[0];

            let routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: path,
            };

            if let Ok(fragments) = self.assembler.fragment_message(&message) {
                for frag in fragments {
                    let packet = Packet {
                        routing_header: routing_header.clone(),
                        session_id,
                        pack_type: PacketType::MsgFragment(frag),
                    };

                    self.packet_cache.insert(packet.clone());

                    info!(
                        "{} [ ChatClient {} ]: Requesting client list from [ Server {} ] with session_id: {}",
                        "ℹ".blue(),
                        self.id,
                        server_id,
                        session_id
                    );

                    self.forward_packet(packet);
                }
            }
        }
    }

    fn register_to(&mut self, server_id: NodeId) {
        if self.running && self.communication_server_list.contains(&server_id) {
            let session_id = self.session_id_counter;
            self.session_id_counter += 1;
            let message = Message::new_client_message(
                session_id,
                self.id,
                server_id,
                ClientMessage::RegisterToChat,
            );

            let path = self.network_knowledge.find_paths_to(self.id, server_id)[0];

            let routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: path,
            };

            if let Ok(fragments) = self.assembler.fragment_message(&message) {
                for frag in fragments {
                    let packet = Packet {
                        routing_header: routing_header.clone(),
                        session_id,
                        pack_type: PacketType::MsgFragment(frag),
                    };

                    self.packet_cache.insert(packet.clone());

                    info!(
                        "{} [ ChatClient {} ]: Registering to [ CommunicationServer {} ] with session_id: {}",
                        "ℹ".blue(),
                        self.id,
                        server_id,
                        session_id
                    );

                    self.forward_packet(packet);
                }
            }
        }
    }

    fn logout(&mut self) {
        if self.running {
            let session_id = self.session_id_counter;
            self.session_id_counter += 1;
            let message =
                Message::new_client_message(session_id, self.id, server_id, ClientMessage::Logout);

            let path = self.network_knowledge.find_paths_to(self.id, server_id)[0];

            let routing_header = SourceRoutingHeader {
                hop_index: 1,
                hops: path,
            };

            if let Ok(fragments) = self.assembler.fragment_message(&message) {
                for frag in fragments {
                    let packet = Packet {
                        routing_header: routing_header.clone(),
                        session_id,
                        pack_type: PacketType::MsgFragment(frag),
                    };

                    self.packet_cache.insert(packet.clone());

                    info!(
                        "{} [ ChatClient {} ]: Logging out from [ CommunicationServer {} ] with session_id: {}",
                        "ℹ".blue(),
                        self.id,
                        server_id,
                        session_id
                    );

                    self.forward_packet(packet);
                }
            }
        } else {
            // error not running
        }
    }
}
