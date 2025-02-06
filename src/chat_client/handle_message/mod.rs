use super::ChatClient;

use colored::Colorize;
use log::{error, info};

use messages::{client_commands::*, high_level_messages::*};

use wg_2024::network::NodeId;

impl ChatClient {
    pub(crate) fn query_communication_servers(&mut self) {
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

    pub(crate) fn generate_and_send_message(
        &mut self,
        message_content: MessageContent,
        destination: NodeId,
    ) {
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

    pub(crate) fn is_running(&self) -> bool {
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

    pub(crate) fn is_registered(&self) -> bool {
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

    pub(crate) fn read_message(&mut self) {
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
}
