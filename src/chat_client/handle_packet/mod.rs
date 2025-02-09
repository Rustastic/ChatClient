use super::ChatClient;
use colored::Colorize;
use log::{error, info, warn};

use messages::client_commands::ChatClientEvent;
use rand::Rng;
use wg_2024::{
    network::{NodeId, SourceRoutingHeader},
    packet::{
        Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType,
    },
};
mod read_message;
impl ChatClient {
    pub(super) fn handle_packet(&mut self, packet: Packet) {
        if let PacketType::FloodRequest(mut flood_request) = packet.clone().pack_type {
            flood_request.path_trace.push((self.id, NodeType::Client));

            let mut routing_header = SourceRoutingHeader::new(
                flood_request
                    .clone()
                    .path_trace
                    .into_iter()
                    .map(|(id, _ntype)| id)
                    .collect(),
                1,
            );

            routing_header.hops.reverse();

            match routing_header.current_hop() {
                Some(dest) => {
                    self.send_flood_response(
                        dest,
                        &flood_request,
                        routing_header,
                        packet.session_id,
                        "Reached a ChatClient",
                    );
                }
                None => {
                    error!(
                        "{} [ ChatClient {} ]: No destination found in routing header",
                        "✗".red(),
                        self.id
                    );
                }
            }
        } else if self.check_packet_correct_id(packet.clone()) {
            if packet.routing_header.hop_index == packet.routing_header.len() - 1 {
                info!(
                    "{} [ ChatClient {} ]: received a packet from [ Node {} ]",
                    "✓".green(),
                    self.id,
                    packet.routing_header.hops[packet.routing_header.hop_index - 1]
                );
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
                error!(
                    "{} [ ChatClient {} ]: received a packet but it is not the destination",
                    "✗".red(),
                    self.id
                );

                if let PacketType::MsgFragment(fragment) = packet.clone().pack_type {
                    self.send_nack(
                        packet,
                        Some(fragment),
                        NackType::UnexpectedRecipient(self.id),
                    );
                } else {
                    self.controller_send
                        .send(ChatClientEvent::ControllerShortcut(packet))
                        .unwrap();
                }
            }
        }
    }

    fn check_packet_correct_id(&self, packet: Packet) -> bool {
        if self.id == packet.routing_header.hops[packet.routing_header.hop_index] {
            true
        } else {
            error!(
                "{} [ ChatClient {} ]: does not correspond to the Node indicated by the `hop_index`, routing_header: {} packetype: {}",
                "✗".red(),
                self.id,
                packet.routing_header,
                packet.pack_type
            );

            if let PacketType::MsgFragment(frag) = packet.clone().pack_type {
                self.send_nack(packet, Some(frag), NackType::UnexpectedRecipient(self.id));
            } else {
                self.controller_send
                    .send(ChatClientEvent::ControllerShortcut(packet))
                    .unwrap();
            }

            false
        }
    }

    pub(crate) fn forward_packet(&self, packet: Packet) -> bool {
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
            if let PacketType::MsgFragment(fragment) = packet_type {
                error!(
                    "{} [ ChatClient {} ]: does not exist in the path",
                    "✗".red(),
                    destination
                );

                self.send_nack(
                    packet,
                    Some(fragment),
                    NackType::ErrorInRouting(destination),
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
                    "└─>{} [ ChatClient {} ]: {} sent to Simulation Controller",
                    "!!!".yellow(),
                    self.id,
                    packet_type,
                );
            }

            false
        }
    }

    fn send_nack(&self, mut packet: Packet, fragment: Option<Fragment>, nack_type: NackType) {
        packet
            .routing_header
            .hops
            .drain(packet.routing_header.hop_index..);

        if let NackType::UnexpectedRecipient(id) = nack_type {
            packet.routing_header.hops.push(id);
        }

        packet.routing_header.hops.reverse();

        packet.routing_header.hop_index = 1;

        let prev_hop = packet.routing_header.current_hop().unwrap();

        let nack = Nack {
            fragment_index: match fragment {
                Some(frag) => frag.fragment_index,
                None => 0,
            }, // Default fragment index for non-fragmented NACKs
            nack_type,
        };

        packet.pack_type = PacketType::Nack(nack);

        if let Some(sender) = self.packet_send.get(&prev_hop) {
            // Send the NACK to the previous hop
            match sender.send(packet.clone()) {
                Ok(()) => {
                    warn!(
                        "{} Nack was sent from [ ChatClient {} ] to [ Drone {} ]",
                        "!!!".yellow(),
                        self.id,
                        prev_hop
                    );
                }
                Err(e) => {
                    // Handle failure to send the NACK, send to the simulation controller instead
                    warn!(
                        "{} [ ChatClient {} ]: Failed to send the Nack to [ Drone {} ]: {}",
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
}
