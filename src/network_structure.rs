use std::{collections::{HashMap, HashSet}, thread::current};

use wg_2024::{network::NodeId, packet::NodeType};

// Rappresenta un nodo nella rete
#[derive(Debug, Clone)]
struct NetworkNode {
    id: NodeId,
    node_type: NodeType,
    neighbors: HashSet<NodeId>,
}

struct NetworkTopology {
    nodes: HashMap<NodeId, NetworkNode>,
}

impl NetworkTopology {
    //  itera sulla path trace, inserisce nella hash map i nodi non presenti e aggiunge tutti i vicini (bidirezionale)
    pub fn process_path_trace(&mut self, flood_id: u64, path_trace: Vec<(NodeId, NodeType)>) {
        for i in 0..path_trace.len() {
            let (node_id, node_type) = path_trace[i];

            let node = self.nodes.entry(node_id).or_insert_with(|| NetworkNode {
                id: node_id,
                node_type: node_type.clone(),
                neighbors: HashSet::new(),
            });

            //parte da zero (dal client che ha inizializzato il flooding) quandi si torva al secondo nodo
            // inserisce il collegamento con il primo nodo e inserisce il secondo nodo nei vicini del primo
            if i > 0 {
                let (prev_id, _) = path_trace[i - 1];
                node.neighbors.insert(prev_id);
                if let Some(prev_node) = self.nodes.get_mut(&prev_id) {
                    prev_node.neighbors.insert(node_id);
                }
            }
        }
    }

    pub fn find_paths_to(&self, current_id: NodeId, destination_id: NodeId) -> Vec<Vec<NodeId>> {
        let mut paths = Vec::new();
        let mut visited = HashSet::new();
        let mut current_path = vec![current_id];

        self.dfs(
            current_id,
            destination_id,
            &mut visited,
            &mut current_path,
            &mut paths,
        );

        paths
    }

    fn dfs(
        &self,
        current: u8,
        destination: u8,
        visited: &mut HashSet<u8>,
        current_path: &mut Vec<u8>,
        paths: &mut Vec<Vec<u8>>,
    ) {
        if current == destination {
            paths.push(current_path.clone());
            return;
        }

        visited.insert(current);

        if let Some(node) = self.nodes.get(&current) {
            for &neighbor in &node.neighbors {
                if !visited.contains(&neighbor) {
                    current_path.push(neighbor);
                    self.dfs(neighbor, destination, visited, current_path, paths);
                    current_path.pop();
                }
            }
        }

        visited.remove(&current);
    }
}


