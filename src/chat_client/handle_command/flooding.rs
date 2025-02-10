use colored::Colorize;
use log::{error, info};

use crate::ChatClient;

impl ChatClient {
    pub(super) fn start_flooding(&mut self) {
        let flood_request_packet = self.router.get_flood_request();

        for (neighbor, channel) in &self.packet_send {
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
}
