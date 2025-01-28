use crate::network_structure::NetworkTopology;
use assembler::Assembler;
use colored::Colorize;
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{error, info, warn};
use messages::{Message, MessageContent};
use std::collections::{HashMap, HashSet};
use wg_2024::{
    controller::{DroneCommand, DroneEvent},
    network::{NodeId, SourceRoutingHeader},
    packet::{
        self, Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType
    },
};

pub struct ChatClient {

    id: NodeId,
    assembler: Assembler,
    network_knowledge: NetworkTopology,
    client_list: Vec<NodeId>,
    message_buffer: Vec<Message>,

    controller_send: Sender<DroneEvent>,
    controller_recv: Receiver<DroneCommand>,
    packet_recv: Receiver<Packet>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    flood_id_received: HashSet<(u64, NodeId)>,

    flood_id_counter: u64,
    session_id_counter: u64,
    
}

impl ChatClient {
    pub fn new() {
        todo!()
    }
    pub fn run(&mut self) {
        loop {
            select_biased! {
                recv(self.controller_recv) -> command => {
                    if let Ok(command) = command {
                        match command {
                            DroneCommand::Crash => {
                                warn!("{} [ ChatClient {} ]: Has crashed", "!!!".yellow(), self.id);
                                break;
                            },
                            _ => todo!()
                        }
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
                    PacketType::Ack(ack) => self.process_ack(ack),
                    PacketType::Nack(nack) => self.process_nack(nack, &packet),
                    PacketType::FloodResponse(flood_response) => self.process_flood_response(flood_response),
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

        let routing_header = SourceRoutingHeader {
            hops: vec![],
            hop_index: 0,
        };

        let session_id = 0;

        for neighbor in self.packet_send.iter() {
            self.send_flood_request(neighbor, &flood_request, routing_header.clone(), session_id);
        }
    }

    fn process_flood_response(&mut self, flood_response: FloodResponse) {
        //in questo modo viene memorizzato il grafo
        self.network_knowledge.process_path_trace(flood_response.path_trace.clone());
        //missing logging
    }

    fn process_fragment(&mut self, fragment: Fragment, packet: &Packet) {

        let new_routing_header = packet.routing_header.get_reversed();

        let ack_packet = Packet {
            routing_header: new_routing_header,
            session_id: self.session_id_counter,
            pack_type: PacketType::Ack(Ack { fragment_index: fragment.fragment_index })
        };

        self.forward_packet(ack_packet);

        let source_id = packet.routing_header.source().unwrap();

        if let Some(message) = self.assembler.process_fragment(fragment, packet.session_id, source_id) {
            //utilizzo un buffer perchè potrebbero arrivari più messaggi in contemporanea e alcuni potrebbero essere 
            //persi
            self.message_buffer.push(message);
            self.read_message();
        }

        //the client needs to send a ack

        //missing logging

    }

    fn process_ack(&mut self, ack: Ack) {
        //logging

    }

    fn process_nack(&mut self, nack: Nack, packet: &Packet) {
        //logging 
        match nack.clone().nack_type {
            NackType::ErrorInRouting(_) => todo!(),
            NackType::DestinationIsDrone => todo!(),
            NackType::Dropped => todo!(),
            NackType::UnexpectedRecipient(_) => todo!(),
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
                    
                    messages::ServerMessage::ClientList(items) => {
                        self.client_list = items;
                         
                        info!(
                            "{} [ ChatClient {} ]: Updated client list: {:?}",
                            "ℹ".blue(),
                            self.id,
                            self.client_list
                        );
                    },
                    messages::ServerMessage::MessageReceived { sender_id, content } => {
                        
                        info!(
                            "{} [ ChatClient {} ]: Message received from [ Client {} ]: {}",
                            "✓".green(),
                            self.id,
                            sender_id,
                            content
                        );

                    },
                    messages::ServerMessage::ErrorWrongClientId => {
                        error!(
                            "{} [ ChatClient {} ]: Received an error indicating wrong client ID",
                            "✗".red(),
                            self.id
                        );
                    }
                    _ => {
                        
                        error!(
                            "{} [ ChatClient {} ]: Received a message intended for a web browser",
                            "✗".red(),
                            self.id
                        );
                    }
                }
            }
        } else {
            //log that there are no messages to read
            info!("{} [ ChatClient {} ]: No messages to read", "ℹ".blue(), self.id);
        }
    }
}

// flooding
// ricevere pacchetti
// interazioni con il simulation controller
// invio di messaggi di alto livello
// ricevimento di messaggi di alto livello
