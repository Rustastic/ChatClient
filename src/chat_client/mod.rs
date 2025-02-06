use assembler::HighLevelMessageFactory;
use colored::Colorize;
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{error, info, warn};
use source_routing::Router;

use messages::{client_commands::*, high_level_messages::*};

use rand::Rng;
use std::{
    collections::{HashMap, HashSet},
    thread::panicking,
};
use wg_2024::{
    network::{NodeId, SourceRoutingHeader},
    packet::{
        Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType,
    },
};

mod handle_message;
mod process_packet;

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
}
