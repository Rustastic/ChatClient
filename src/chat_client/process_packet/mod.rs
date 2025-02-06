use super::ChatClient;
use colored::Colorize;
use log::{error, info};

use rand::Rng;
use wg_2024::{network::SourceRoutingHeader, packet::{Ack, FloodResponse, Fragment, Nack, NackType, Packet, PacketType}};

impl ChatClient {
    pub(crate) fn handle_packet(&mut self, mut packet: Packet) {
        if let PacketType::FloodRequest(flood_request) = packet.clone().pack_type {
            let routing_header = SourceRoutingHeader::with_first_hop(
                flood_request
                    .clone()
                    .path_trace
                    .into_iter()
                    .map(|(id, _ntype)| id)
                    .collect(),
            )
            .get_reversed();

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
        } else if self.check_packet_correct_id(packet.clone())
            && packet.routing_header.hop_index == packet.routing_header.len() - 1
        {
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

            let fragment = match packet.clone().pack_type {
                PacketType::MsgFragment(fragment) => Some(fragment),
                _ => None,
            };

            self.send_nack(packet, fragment, NackType::UnexpectedRecipient(self.id));
        }
    }

    pub(crate) fn process_flood_response(&mut self, flood_response: FloodResponse) {
        self.router.handle_flood_response(&flood_response);
        info!(
            "{} [ ChatClient {} ]: Processed FloodResponse with flood_id: {}",
            "✓".green(),
            self.id,
            flood_response.flood_id
        );
    }

    pub(crate) fn process_fragment(&mut self, fragment: Fragment, packet: &Packet) {
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

    pub(crate) fn process_nack(&mut self, nack: Nack, packet: &Packet) {
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
