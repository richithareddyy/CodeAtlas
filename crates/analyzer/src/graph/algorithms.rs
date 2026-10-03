//! Graph algorithms over a compact adjacency-list digraph.
//!
//! Nodes are dense indices `0..n`. Nothing here knows about code; the
//! [`CodeGraph`](super::CodeGraph) builds projections (call graph, module
//! graph, file graph) and maps results back to symbols. All algorithms are
//! iterative, so deep graphs cannot overflow the stack, and deterministic:
//! successor lists are sorted and results come out in a stable order.

use std::collections::VecDeque;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Digraph {
    adj: Vec<Vec<usize>>,
}

impl Digraph {
    pub fn new(nodes: usize) -> Self {
        Self {
            adj: vec![Vec::new(); nodes],
        }
    }

    /// Builds a graph from an edge list; duplicate edges are merged.
    pub fn from_edges(nodes: usize, edges: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let mut graph = Self::new(nodes);
        for (from, to) in edges {
            graph.adj[from].push(to);
        }
        for successors in &mut graph.adj {
            successors.sort_unstable();
            successors.dedup();
        }
        graph
    }

    pub fn len(&self) -> usize {
        self.adj.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adj.is_empty()
    }

    pub fn successors(&self, node: usize) -> &[usize] {
        &self.adj[node]
    }

    pub fn edge_count(&self) -> usize {
        self.adj.iter().map(Vec::len).sum()
    }

    pub fn reversed(&self) -> Digraph {
        Digraph::from_edges(
            self.len(),
            self.adj
                .iter()
                .enumerate()
                .flat_map(|(from, tos)| tos.iter().map(move |&to| (to, from))),
        )
    }
}

/// Result of a breadth-first search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BfsTree<E> {
    /// Reached nodes in visiting order, excluding the start nodes.
    pub order: Vec<usize>,
    /// Distance from the nearest start node.
    pub depth: Vec<Option<u32>>,
    /// Predecessor and the label of the edge that first reached the node.
    pub parent: Vec<Option<(usize, E)>>,
    /// Set when `max_nodes` stopped the search before it was exhausted.
    pub truncated: bool,
}

impl<E: Clone> BfsTree<E> {
    /// Edges from a start node to `node`, in order.
    pub fn path_to(&self, node: usize) -> Vec<(usize, usize, E)> {
        let mut path = Vec::new();
        let mut current = node;
        while let Some((prev, label)) = &self.parent[current] {
            path.push((*prev, current, label.clone()));
            current = *prev;
        }
        path.reverse();
        path
    }
}

/// Breadth-first search from `starts`, up to `max_depth` steps and
/// `max_nodes` reached nodes. `neighbours(node, depth)` yields
/// `(next, edge_label)` pairs; the label of the first edge reaching a node
/// is kept for evidence. Iteration order of `neighbours` decides ties.
pub fn bfs<E, F, I>(
    nodes: usize,
    starts: &[usize],
    max_depth: u32,
    max_nodes: usize,
    mut neighbours: F,
) -> BfsTree<E>
where
    E: Clone,
    F: FnMut(usize, u32) -> I,
    I: IntoIterator<Item = (usize, E)>,
{
    let mut tree = BfsTree {
        order: Vec::new(),
        depth: vec![None; nodes],
        parent: vec![None; nodes],
        truncated: false,
    };
    let mut queue = VecDeque::new();
    for &start in starts {
        if tree.depth[start].is_none() {
            tree.depth[start] = Some(0);
            queue.push_back(start);
        }
    }
    while let Some(node) = queue.pop_front() {
        let depth = tree.depth[node].unwrap_or(0);
        if depth >= max_depth {
            continue;
        }
        for (next, label) in neighbours(node, depth) {
            if tree.depth[next].is_some() {
                continue;
            }
            if tree.order.len() >= max_nodes {
                tree.truncated = true;
                return tree;
            }
            tree.depth[next] = Some(depth + 1);
            tree.parent[next] = Some((node, label));
            tree.order.push(next);
            queue.push_back(next);
        }
    }
    tree
}

/// Shortest path (fewest edges) from `from` to `to`, inclusive.
pub fn shortest_path(graph: &Digraph, from: usize, to: usize) -> Option<Vec<usize>> {
    if from == to {
        return Some(vec![from]);
    }
    let tree = bfs(graph.len(), &[from], u32::MAX, usize::MAX, |n, _| {
        graph.successors(n).iter().map(|&s| (s, ()))
    });
    tree.depth[to]?;
    let mut path: Vec<usize> = tree.path_to(to).into_iter().map(|(a, _, _)| a).collect();
    path.push(to);
    Some(path)
}

/// Strongly connected components (Tarjan), iteratively. Components come
/// out in reverse topological order of the condensation (a component is
/// emitted after every component it can reach). Members are sorted.
pub fn tarjan_scc(graph: &Digraph) -> Vec<Vec<usize>> {
    const UNVISITED: usize = usize::MAX;
    let n = graph.len();
    let mut index = vec![UNVISITED; n];
    let mut lowlink = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut components = Vec::new();
    let mut next_index = 0;

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        // (node, position of the next successor to examine)
        let mut work = vec![(root, 0usize)];
        index[root] = next_index;
        lowlink[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;

        while let Some(&mut (node, ref mut pos)) = work.last_mut() {
            if let Some(&succ) = graph.successors(node).get(*pos) {
                *pos += 1;
                if index[succ] == UNVISITED {
                    index[succ] = next_index;
                    lowlink[succ] = next_index;
                    next_index += 1;
                    stack.push(succ);
                    on_stack[succ] = true;
                    work.push((succ, 0));
                } else if on_stack[succ] {
                    lowlink[node] = lowlink[node].min(index[succ]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                lowlink[parent] = lowlink[parent].min(lowlink[node]);
            }
            if lowlink[node] == index[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort_unstable();
                components.push(component);
            }
        }
    }
    components
}

/// Components that contain a cycle: more than one node, or a self-loop.
/// Sorted by size (largest first), then by smallest member.
pub fn cyclic_components(graph: &Digraph) -> Vec<Vec<usize>> {
    let mut cycles: Vec<Vec<usize>> = tarjan_scc(graph)
        .into_iter()
        .filter(|c| c.len() > 1 || graph.successors(c[0]).contains(&c[0]))
        .collect();
    cycles.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    cycles
}

/// A shortest cycle through the smallest member of a cyclic component,
/// as nodes in order (the first node is not repeated at the end).
pub fn shortest_cycle(graph: &Digraph, component: &[usize]) -> Vec<usize> {
    let Some(&start) = component.iter().min() else {
        return Vec::new();
    };
    if graph.successors(start).contains(&start) {
        return vec![start];
    }
    let mut inside = vec![false; graph.len()];
    for &m in component {
        inside[m] = true;
    }
    // Search for the shortest path from any successor of `start` back to it.
    let firsts: Vec<usize> = graph
        .successors(start)
        .iter()
        .copied()
        .filter(|&s| inside[s])
        .collect();
    let tree = bfs(graph.len(), &firsts, u32::MAX, usize::MAX, |n, _| {
        graph
            .successors(n)
            .iter()
            .copied()
            .filter(|&s| inside[s])
            .map(|s| (s, ()))
            .collect::<Vec<_>>()
    });
    if tree.depth[start].is_none() {
        return component.to_vec();
    }
    let mut cycle = vec![start];
    let mut back: Vec<usize> = tree.path_to(start).into_iter().map(|(a, _, _)| a).collect();
    cycle.append(&mut back);
    cycle
}

/// Topological order (dependencies after dependents, i.e. edge sources
/// before targets) or, if the graph has cycles, its cyclic components.
pub fn topological_order(graph: &Digraph) -> Result<Vec<usize>, Vec<Vec<usize>>> {
    let n = graph.len();
    let mut in_degree = vec![0usize; n];
    for node in 0..n {
        for &s in graph.successors(node) {
            in_degree[s] += 1;
        }
    }
    let mut ready: std::collections::BTreeSet<usize> =
        (0..n).filter(|&v| in_degree[v] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(node) = ready.pop_first() {
        order.push(node);
        for &s in graph.successors(node) {
            in_degree[s] -= 1;
            if in_degree[s] == 0 {
                ready.insert(s);
            }
        }
    }
    if order.len() == n {
        Ok(order)
    } else {
        Err(cyclic_components(graph))
    }
}

/// Dependency layers. Edges mean "depends on". Layer 0 holds nodes that
/// depend on nothing; every other node sits one layer above the highest
/// layer it depends on. Nodes of one cyclic component share a layer.
pub fn dependency_layers(graph: &Digraph) -> Vec<u32> {
    let components = tarjan_scc(graph);
    let mut component_of = vec![0usize; graph.len()];
    for (c, members) in components.iter().enumerate() {
        for &m in members {
            component_of[m] = c;
        }
    }
    // Tarjan emits a component after everything it reaches, so dependencies
    // already have their layer when a component is processed.
    let mut component_layer = vec![0u32; components.len()];
    for (c, members) in components.iter().enumerate() {
        let layer = members
            .iter()
            .flat_map(|&m| graph.successors(m))
            .map(|&s| component_of[s])
            .filter(|&sc| sc != c)
            .map(|sc| component_layer[sc] + 1)
            .max()
            .unwrap_or(0);
        component_layer[c] = layer;
    }
    (0..graph.len())
        .map(|v| component_layer[component_of[v]])
        .collect()
}

/// In-degree and out-degree of every node.
pub fn degrees(graph: &Digraph) -> (Vec<usize>, Vec<usize>) {
    let mut in_degree = vec![0; graph.len()];
    let out_degree = (0..graph.len())
        .map(|v| graph.successors(v).len())
        .collect();
    for v in 0..graph.len() {
        for &s in graph.successors(v) {
            in_degree[s] += 1;
        }
    }
    (in_degree, out_degree)
}

/// Betweenness centrality (Brandes, unweighted, directed, unnormalised):
/// for each node, the number of shortest paths between other pairs that
/// pass through it, with ties split evenly.
pub fn betweenness(graph: &Digraph) -> Vec<f64> {
    let n = graph.len();
    let mut centrality = vec![0.0; n];
    let mut stack = Vec::with_capacity(n);
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut sigma = vec![0.0f64; n];
    let mut distance = vec![-1i64; n];
    let mut delta = vec![0.0f64; n];
    let mut queue = VecDeque::new();

    for source in 0..n {
        stack.clear();
        for v in 0..n {
            predecessors[v].clear();
            sigma[v] = 0.0;
            distance[v] = -1;
            delta[v] = 0.0;
        }
        sigma[source] = 1.0;
        distance[source] = 0;
        queue.push_back(source);
        while let Some(v) = queue.pop_front() {
            stack.push(v);
            for &w in graph.successors(v) {
                if distance[w] < 0 {
                    distance[w] = distance[v] + 1;
                    queue.push_back(w);
                }
                if distance[w] == distance[v] + 1 {
                    sigma[w] += sigma[v];
                    predecessors[w].push(v);
                }
            }
        }
        while let Some(w) = stack.pop() {
            for &v in &predecessors[w] {
                delta[v] += sigma[v] / sigma[w] * (1.0 + delta[w]);
            }
            if w != source {
                centrality[w] += delta[w];
            }
        }
    }
    centrality
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(usize, usize)]) -> Digraph {
        Digraph::from_edges(n, edges.iter().copied())
    }

    #[test]
    fn bfs_records_depth_parents_and_limits() {
        // 0 -> 1 -> 2 -> 3, 0 -> 2
        let g = graph(4, &[(0, 1), (1, 2), (2, 3), (0, 2)]);
        let tree = bfs(4, &[0], 10, 10, |n, _| {
            g.successors(n).iter().map(move |&s| (s, (n, s)))
        });
        assert_eq!(tree.depth, vec![Some(0), Some(1), Some(1), Some(2)]);
        assert_eq!(tree.order, vec![1, 2, 3]);
        assert_eq!(tree.path_to(3), vec![(0, 2, (0, 2)), (2, 3, (2, 3))]);

        let shallow = bfs(4, &[0], 1, 10, |n, _| {
            g.successors(n).iter().map(|&s| (s, ()))
        });
        assert_eq!(shallow.order, vec![1, 2]);
        let capped = bfs(4, &[0], 10, 1, |n, _| {
            g.successors(n).iter().map(|&s| (s, ()))
        });
        assert_eq!(capped.order, vec![1]);
        assert!(capped.truncated);
    }

    #[test]
    fn shortest_path_prefers_fewest_edges() {
        let g = graph(5, &[(0, 1), (1, 2), (2, 4), (0, 3), (3, 4)]);
        assert_eq!(shortest_path(&g, 0, 4), Some(vec![0, 3, 4]));
        assert_eq!(shortest_path(&g, 4, 0), None);
        assert_eq!(shortest_path(&g, 2, 2), Some(vec![2]));
    }

    #[test]
    fn tarjan_finds_components_in_reverse_topological_order() {
        // {0,1,2} cycle -> {3,4} cycle -> 5; 6 isolated with a self-loop.
        let g = graph(
            7,
            &[
                (0, 1),
                (1, 2),
                (2, 0),
                (2, 3),
                (3, 4),
                (4, 3),
                (4, 5),
                (6, 6),
            ],
        );
        let sccs = tarjan_scc(&g);
        assert_eq!(sccs, vec![vec![5], vec![3, 4], vec![0, 1, 2], vec![6]]);
        assert_eq!(
            cyclic_components(&g),
            vec![vec![0, 1, 2], vec![3, 4], vec![6]]
        );
    }

    #[test]
    fn tarjan_handles_deep_chains_without_recursion() {
        let n = 200_000;
        let g = graph(n, &(0..n - 1).map(|i| (i, i + 1)).collect::<Vec<_>>());
        assert_eq!(tarjan_scc(&g).len(), n);
    }

    #[test]
    fn shortest_cycle_through_component() {
        // 0 -> 1 -> 2 -> 0 and the chord 0 -> 2.
        let g = graph(3, &[(0, 1), (1, 2), (2, 0), (0, 2)]);
        assert_eq!(shortest_cycle(&g, &[0, 1, 2]), vec![0, 2]);
        let self_loop = graph(1, &[(0, 0)]);
        assert_eq!(shortest_cycle(&self_loop, &[0]), vec![0]);
    }

    #[test]
    fn topological_order_or_cycles() {
        let dag = graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        assert_eq!(topological_order(&dag), Ok(vec![0, 1, 2, 3]));
        let cyclic = graph(3, &[(0, 1), (1, 0), (1, 2)]);
        assert_eq!(topological_order(&cyclic), Err(vec![vec![0, 1]]));
    }

    #[test]
    fn layers_put_foundations_first_and_collapse_cycles() {
        // app(0) -> service(1) -> {repo(2) <-> cache(3)} -> util(4)
        let g = graph(5, &[(0, 1), (1, 2), (2, 3), (3, 2), (3, 4)]);
        assert_eq!(dependency_layers(&g), vec![3, 2, 1, 1, 0]);
    }

    #[test]
    fn degree_and_betweenness() {
        // Path 0 -> 1 -> 2: only 1 lies between (0, 2).
        let path = graph(3, &[(0, 1), (1, 2)]);
        assert_eq!(degrees(&path), (vec![0, 1, 1], vec![1, 1, 0]));
        assert_eq!(betweenness(&path), vec![0.0, 1.0, 0.0]);

        // Diamond 0 -> {1, 2} -> 3: the two middle nodes split the path.
        let diamond = graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        assert_eq!(betweenness(&diamond), vec![0.0, 0.5, 0.5, 0.0]);
    }
}
