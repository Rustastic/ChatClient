use colored::Colorize;
use log::{error, info, warn};
use messages::{
    client_commands::{ChatClientCommand, ChatClientEvent},
    high_level_messages::{ClientMessage, MessageContent},
};

use super::ChatClient;

mod flooding;
mod send_message;

impl ChatClient {
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle_command(&mut self, command: ChatClientCommand) {
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
                    info!("{:?}", self.communication_server_list);
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
                            "{} [ ChatClient {} ]: Cannot register to server {}, it is not a communication server, communication_server_list: {:?}",
                            "✗".red(),
                            self.id,
                            server_id,
                            self.communication_server_list
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
            ChatClientCommand::LogNetwork => {
                self.router.log_network();
            }
        }
    }
}
