use hashlink::LinkedHashSet;

use crate::graphs::base_edge::BaseEdgeStruct;
use crate::graphs::seq_graph::SeqGraph;
use crate::haplotype::haplotype::Haplotype;
use crate::read_threading::abstract_read_threading_graph::AbstractReadThreadingGraph;
use crate::utils::simple_interval::Locatable;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Status {
    Failed,
    JustAssembledReference,
    AssembledSomeVariation,
}

/**
 * Result of assembling, with the resulting graph and status
 */
#[derive(Debug, Clone)]
pub struct AssemblyResult<L: Locatable, A: AbstractReadThreadingGraph> {
    pub(crate) status: Status,
    pub(crate) threading_graph: Option<A>,
    pub(crate) graph: Option<SeqGraph<BaseEdgeStruct>>,
    pub(crate) discovered_haplotypes: LinkedHashSet<Haplotype<L>>,
    pub(crate) contains_suspect_haploptypes: bool,
}

impl<L: Locatable, A: AbstractReadThreadingGraph> Eq for AssemblyResult<L, A> {}

impl<L: Locatable, A: AbstractReadThreadingGraph> PartialEq for AssemblyResult<L, A> {
    fn eq(&self, other: &Self) -> bool {
        self.status == other.status && self.discovered_haplotypes == other.discovered_haplotypes
    }
}

impl<L: Locatable, A: AbstractReadThreadingGraph> AssemblyResult<L, A> {
    /**
     * Create a new assembly result
     * @param status the status, cannot be null
     * @param graph the resulting graph of the assembly, can only be null if result is failed
     */
    pub fn new(
        status: Status,
        graph: Option<SeqGraph<BaseEdgeStruct>>,
        threading_graph: Option<A>,
    ) -> AssemblyResult<L, A> {
        AssemblyResult {
            status,
            graph,
            threading_graph,
            discovered_haplotypes: LinkedHashSet::new(),
            contains_suspect_haploptypes: false,
        }
    }

    pub fn set_discovered_haplotypes(
        &mut self,
        discovered_haplotypes: LinkedHashSet<Haplotype<L>>,
    ) {
        self.discovered_haplotypes = discovered_haplotypes
    }

    pub fn set_contains_suspect_haplotypes(&mut self, contains_suspect_haplotypes: bool) {
        self.contains_suspect_haploptypes = contains_suspect_haplotypes
    }

    pub fn get_kmer_size(&self) -> usize {
        match &self.graph {
            None => match &self.threading_graph {
                None => panic!("No established kmer size for assembly result"),
                Some(threading_graph) => threading_graph.get_kmer_size(),
            },
            Some(graph) => graph.base_graph.get_kmer_size(),
        }
    }

    pub fn get_seq_graph(self) -> Option<SeqGraph<BaseEdgeStruct>> {
        self.graph
    }
}
