use colored::Colorize;
use log::{error, info};
use messages::{
    client_commands::ChatClientEvent,
    high_level_messages::{MessageContent, ServerMessage, ServerType},
};

use crate::ChatClient;

impl ChatClient {
    pub(super) fn read_message(&mut self) {
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
                            .send(ChatClientEvent::ClientList(
                                self.id,
                                self.client_list.clone(),
                            ))
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
                            .send(ChatClientEvent::MessageReceived(
                                sender_id, self.id, content,
                            ))
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
