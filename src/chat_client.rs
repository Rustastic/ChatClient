use assembler::HighLevelMessageFactory;
use colored::Colorize;
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{error, info, warn};
use source_routing::Router;

use messages::{client_commands::*, high_level_messages::*};

use rand::Rng;
use std::collections::{HashMap, HashSet};
use wg_2024::{
    network::{NodeId, SourceRoutingHeader},
    packet::{
        Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType,
    },
};

pub struct ChatClient {
    id: NodeId,
    running: bool,
    registered: Option<NodeId>,
    client_list: Vec<NodeId>,
    msgfactory: HighLevelMessageFactory,
    router: Router,
    communication_server_list: Vec<NodeId>,
    message_buffer: Vec<Message>,

    controller_send: Sender<ChatClientEvent>,
    controller_recv: Receiver<ChatClientCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_id_received: HashSet<(u64, NodeId)>,
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
            msgfactory: HighLevelMessageFactory::new(id, NodeType::Client),
            router: Router::new(id, NodeType::Client),
            client_list: Vec::new(),
            message_buffer: Vec::new(),
            controller_send,
            controller_recv,
            packet_recv,
            packet_send,
            flood_id_received: HashSet::new(),
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
            ChatClientCommand::SendMessageTo(client_id, text) => {
                if self.is_running() && self.is_registered() {
                    if self.client_list.contains(&client_id) {
                        let server_id = self.registered.unwrap();
                        info!(
                            "{} [ ChatClient {} ]: Sending message to [ ChatClient {} ] through [ CommunicationServer {} ]",
                            "ℹ".blue(),
                            self.id,
                            client_id,
                            server_id,
                        );
                        let message_content =
                            MessageContent::FromClient(ClientMessage::SendMessage {
                                recipient_id: client_id,
                                content: text,
                            });
                        self.generate_and_send_message(message_content, server_id);
                    } else {
                        error!(
                            "{} [ ChatClient {} ]: Cannot send message, destination client {} is unreachable",
                            "✗".red(),
                            self.id,
                            client_id
                        );
                        self.controller_send
                            .send(ChatClientEvent::UnreachableClient(client_id))
                            .unwrap();
                    }
                }
            }
            ChatClientCommand::RegisterTo(server_id) => {
                if self.is_running() {
                    if self.communication_server_list.contains(&server_id) {
                        info!(
                            "{} [ ChatClient {} ]: Registering to [ CommunicationServer {} ]",
                            "ℹ".blue(),
                            self.id,
                            server_id,
                        );
                        let message_content =
                            MessageContent::FromClient(ClientMessage::RegisterToChat);
                        self.generate_and_send_message(message_content, server_id);
                    } else {
                        error!(
                            "{} [ ChatClient {} ]: Cannot register to server {}, it is not a communication server",
                            "✗".red(),
                            self.id,
                            server_id
                        );
                    }
                }
            }
            ChatClientCommand::GetClientList => {
                if self.is_running() && self.is_registered() {
                    let server_id = self.registered.unwrap();
                    info!(
                        "{} [ ChatClient {} ]: Requesting client list from [ Server {} ]",
                        "ℹ".blue(),
                        self.id,
                        server_id,
                    );
                    let message_content = MessageContent::FromClient(ClientMessage::GetClientList);
                    self.generate_and_send_message(message_content, server_id);
                }
            }
            ChatClientCommand::LogOut => {
                if self.is_running() && self.is_registered() {
                    let server_id = self.registered.unwrap();
                    info!(
                        "{} [ ChatClient {} ]: Logging out from [ CommunicationServer {} ]",
                        "ℹ".blue(),
                        self.id,
                        server_id,
                    );
                    let message_content = MessageContent::FromClient(ClientMessage::Logout);
                    self.generate_and_send_message(message_content, server_id);
                }
            }
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

            if packet.routing_header.hop_index == packet.routing_header.hops.len() {
                // the client received a packet
                match packet.clone().pack_type {
                    PacketType::MsgFragment(fragment) => self.process_fragment(fragment, &packet),
                    PacketType::Ack(ack) => self.msgfactory.received_ack(ack, packet.session_id),
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
                    "{} [ ChatClient {} ]: can't send packet to Drone {} because it is not its neighbor",
                    "✗".red(),
                    self.id,
                    neighbor
                );
                    self.send_nack(packet, None, NackType::ErrorInRouting(self.id));
                    return;
                }

                info!(
                    "{} Packet with [ session_id: {} ] is being handled from [ ChatClient {} ]",
                    "✓".green(),
                    packet.session_id,
                    self.id
                );

                // Handle packet types: Nack, Ack, MsgFragment, FloodResponse
                match packet.clone().pack_type {
                    PacketType::Nack(_) => {
                        warn!(
                            "{} [ ChatClient {} ]: received a {}",
                            "!!!".yellow(),
                            self.id,
                            packet.pack_type,
                        );
                        self.forward_packet(packet);
                    }
                    PacketType::Ack(_) => {
                        warn!(
                            "{} [ ChatClient {} ]: received a {}",
                            "!!!".yellow(),
                            self.id,
                            packet.pack_type,
                        );
                        self.forward_packet(packet);
                    }
                    PacketType::MsgFragment(fragment) => {
                        info!(
                            "{} [ ChatClient {} ]: forwarded the the fragment [ fragment_index: {} ] of the Packet [ session_id: {} ]",
                            "✓".green(),
                            self.id,
                            fragment.fragment_index,
                            packet.session_id
                        );

                        self.forward_packet(packet);
                    }
                    PacketType::FloodResponse(flood_response) => {
                        self.handle_flood_response(&flood_response, &packet);
                    }
                    _ => unreachable!(),
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
                "{} [ ChatClient {} ]: does not correspond to the Drone indicated by the `hop_index`",
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
                        .send(ChatClientEvent::ControllerShortcut(packet))
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
                    .send(ChatClientEvent::ControllerShortcut(packet))
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
                        .send(ChatClientEvent::ControllerShortcut(packet))
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
                .send(ChatClientEvent::ControllerShortcut(packet))
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
                        .send(ChatClientEvent::ControllerShortcut(new_packet))
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
                .send(ChatClientEvent::ControllerShortcut(new_packet))
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
                        .send(ChatClientEvent::ControllerShortcut(new_packet))
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
                .send(ChatClientEvent::ControllerShortcut(new_packet))
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
        let flood_request_packet = self.router.get_flood_request();

        for (neighbor, channel) in self.packet_send.iter() {
            if let Ok(()) = channel.send(flood_request_packet.clone()) {
                info!(
                    "{} [ ChatClient {} ]: FloodRequest sent to [ Drone {} ]",
                    "✓".green(),
                    self.id,
                    neighbor
                );
            } else {
                error!(
                    "{} [ ChatClient {} ]: Failed to send FloodRequest to [ Drone {} ]",
                    "✗".red(),
                    self.id,
                    neighbor
                );
            }
        }
    }

    fn process_flood_response(&mut self, flood_response: FloodResponse) {
        self.router.handle_flood_response(&flood_response);
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
            session_id: packet.session_id,
            pack_type: PacketType::Ack(Ack {
                fragment_index: fragment.fragment_index,
            }),
        };

        self.forward_packet(ack_packet);

        let source_id = packet.routing_header.source().unwrap();

        if let Some(message) =
            self.msgfactory
                .received_fragment(fragment, packet.session_id, source_id)
        {
            self.message_buffer.push(message);
            self.read_message();
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
                if let Some(incorrect_packet) =
                    //self.packet_cache.remove_and_get(packet.session_id, nack.fragment_index)
                    self
                        .msgfactory
                        .take_packet(packet.session_id, nack.fragment_index)
                {
                    let dest = incorrect_packet.routing_header.destination().unwrap();

                    let _ = self.router.drone_crashed(unreachable_node);

                    if let Ok(new_routing_header) = self.router.get_source_routing_header(dest) {
                        let new_packet = Packet {
                            pack_type: incorrect_packet.pack_type,
                            routing_header: new_routing_header,
                            session_id: incorrect_packet.session_id,
                        };
                        self.msgfactory.insert_packet(&new_packet);
                        info!(
                            "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                            "✓".green(),
                            self.id,
                            new_packet.session_id,
                            nack.fragment_index,
                            dest
                        );

                        self.forward_packet(new_packet);
                    } else {
                        error!(
                            "{} [ ChatClient {} ]: No path to destination [ CommunicationServer {} ]",
                            "✗".red(),
                            self.id,
                            dest
                        );
                    }
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

                if let Some((dropped_packet, number_of_request)) = self
                    .msgfactory
                    .get_packet(packet.session_id, nack.fragment_index)
                {
                    if number_of_request >= 3 {
                        error!(
                            "{} [ ChatClient {} ]: Packet with session_id: {} and fragment_index: {} has been dropped more than 3 times",
                            "✗".red(),
                            self.id,
                            dropped_packet.session_id,
                            nack.fragment_index
                        );

                        let mut packet_to_resend = dropped_packet.clone();

                        let dest = packet_to_resend.routing_header.destination().unwrap();

                        let routing_headers = self.router.get_multiple_source_routing_headers(dest);

                        let random_index = rand::thread_rng().gen_range(0..routing_headers.len());

                        packet_to_resend.routing_header =
                            routing_headers.get(random_index).unwrap().clone();

                        self.msgfactory.insert_packet(&packet_to_resend);

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

                        self.msgfactory.insert_packet(&packet_to_resend);

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
            NackType::UnexpectedRecipient(_problematic_node) => {
                error!(
                    "{} [ ChatClient {} ]: Received a Nack indicating that the recipient was unexpected",
                    "✗".red(),
                    self.id
                );

                if let Some(incorrect_packet) = self
                    .msgfactory
                    .take_packet(packet.session_id, nack.fragment_index)
                {
                    let dest = incorrect_packet.routing_header.destination().unwrap();

                    if let Ok(new_routing_header) = self.router.get_source_routing_header(dest) {
                        let new_packet = Packet {
                            pack_type: incorrect_packet.pack_type,
                            routing_header: new_routing_header,
                            session_id: incorrect_packet.session_id,
                        };
                        self.msgfactory.insert_packet(&new_packet);
                        info!(
                            "{} [ ChatClient {} ]: Forwarding packet with session_id: {} and fragment_index: {} to [ Server {} ]",
                            "✓".green(),
                            self.id,
                            new_packet.session_id,
                            nack.fragment_index,
                            dest
                        );

                        self.forward_packet(new_packet);
                    } // this the case where a drone receives a packet and hops[hop_index] is not equal to the drone id
                }
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
                            .send(ChatClientEvent::UnreachableClient(client_id))
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

    fn query_communication_servers(&mut self) {
        for server_id in self.router.get_server_list().iter() {
            let message_content = MessageContent::FromClient(ClientMessage::GetServerType);
            info!(
                "{} [ ChatClient {} ]: Querying server [ Server {} ]",
                "ℹ".blue(),
                self.id,
                server_id,
            );
            self.generate_and_send_message(message_content, *server_id);
        }
    }

    fn generate_and_send_message(&mut self, message_content: MessageContent, destination: NodeId) {
        if let Ok(source_routing_header) = self.router.get_source_routing_header(destination) {
            for frag_pack in self.msgfactory.get_message_from_message_content(
                message_content,
                &source_routing_header,
                destination,
            ) {
                self.msgfactory.insert_packet(&frag_pack);
                self.forward_packet(frag_pack);
            }
        }
    }

    fn is_running(&self) -> bool {
        if !self.running {
            error!(
                "{} [ ChatClient {} ]: Cannot send message, ChatClient is not running",
                "✗".red(),
                self.id
            );
            self.controller_send
                .send(ChatClientEvent::ErrorNotRunning)
                .unwrap();
            return false;
        }
        true
    }

    fn is_registered(&self) -> bool {
        if self.registered.is_none() {
            error!(
                "{} [ ChatClient {} ]: Cannot send message, not registered to any server",
                "✗".red(),
                self.id
            );
            self.controller_send
                .send(ChatClientEvent::ErrorNotRegistered)
                .unwrap();
            return false;
        }

        true
    }
}
