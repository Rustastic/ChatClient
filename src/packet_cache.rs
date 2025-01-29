use std::collections::HashMap;

use log::error;
use rand::seq::index;
use wg_2024::packet::{Fragment, Packet, PacketType};

pub struct PacketCache {
    // session_id, fragment_id
    cache: HashMap<(u64, u64), (Packet, u64)>,
}

impl PacketCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    pub fn insert(&mut self, packet: Packet) {
        //the packet will be copied here

        let session_id = packet.session_id;

        if let PacketType::MsgFragment(fragment) = packet.clone().pack_type {
            let fragment_index = fragment.fragment_index;

            self.cache.insert((session_id, fragment_index), (packet, 0));
        } else {
            //log an error: trying to insert a packet that doesnt contain a fragment
            error!("Trying to insert a packet that doesn't contain a fragment");
        }
    }

    pub fn get(&mut self, session_id: u64, fragment_index: u64) -> Option<&(Packet, u64)> {
        // this function should return a reference to the packet and the number of times it has been requested
        // it should also increment the number of times the packet has been requested
        match self.cache.get(&(session_id, fragment_index)) {
            Some((packet, counter)) => {
                let counter = counter + 1;
                self.cache
                    .insert((session_id, fragment_index), (packet.clone(), counter));
                Some(&self.cache[&(session_id, fragment_index)])
            }
            None => None,
        }
    }

    pub fn remove(&mut self, session_id: u64, fragment_index: u64) -> bool {
        match self.cache.remove(&(session_id, fragment_index)) {
            Some(_) => true,
            None => false,
        }
    }

    // i want to combine the remove and the get i.e i want to remove the packet and return it
    pub fn remove_and_get(
        &mut self,
        session_id: u64,
        fragment_index: u64,
    ) -> Option<(Packet, u64)> {
        self.cache.remove(&(session_id, fragment_index))
    }
}
