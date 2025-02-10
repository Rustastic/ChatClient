use super::ChatClient;

use colored::Colorize;
use log::{error, info};

use messages::{
    client_commands::ChatClientEvent,
    high_level_messages::{ClientMessage, MessageContent},
};

use wg_2024::network::NodeId;

impl ChatClient {
    pub(super) fn query_communication_servers(&mut self) {
        for server_id in &self.router.get_server_list() {
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

    pub(super) fn generate_and_send_message(
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

    pub(super) fn is_running(&self) -> bool {
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

    pub(super) fn is_registered(&self) -> bool {
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
