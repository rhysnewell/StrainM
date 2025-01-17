use bird_tool_utils::command::finish_command_safely;
use hashlink::{LinkedHashMap, LinkedHashSet};
use ndarray::{Array, Array1, Array2};
use ndarray_npy::{read_npy, write_npy};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::create_dir_all;
use std::sync::{Arc, Mutex};

use crate::bam_parsing::FlagFilter;
use crate::annotator::variant_annotation::VariantAnnotations;
use crate::genotype::genotype_builder::AttributeObject;
use crate::linkage::linkage_engine::LinkageEngine;
use crate::model::variant_context::VariantContext;
use crate::processing::lorikeet_engine::Elem;
use crate::reference::reference_reader::ReferenceReader;
use crate::utils::simple_interval::Locatable;

/// HaplotypeClusteringEngine provides a suite of functions that takes a list of VariantContexts
/// And clusters them using the flight python module. It will then read in the results of flight
/// and modify the variant contexts to contain their allocated strain.
pub struct HaplotypeClusteringEngine<'a> {
    output_prefix: &'a str,
    variants: Vec<VariantContext>,
    ref_idx: usize,
    ref_name: &'a str,
    n_samples: usize,
    allowed_threads: usize,
    labels: Array1<i32>,
    labels_set: HashSet<i32>,
    cluster_separation: Array2<f64>,
    previous_groups: HashMap<i32, i32>,
    exclusive_groups: HashMap<i32, HashSet<i32>>,
}

impl<'a> HaplotypeClusteringEngine<'a> {
    pub fn new(
        output_prefix: &'a str,
        variants: Vec<VariantContext>,
        reference_reader: &'a ReferenceReader,
        ref_idx: usize,
        n_samples: usize,
        allowed_threads: usize,
    ) -> HaplotypeClusteringEngine<'a> {
        Self {
            output_prefix,
            variants,
            ref_idx,
            ref_name: &reference_reader.genomes_and_contigs.genomes[ref_idx],
            n_samples,
            allowed_threads,
            labels: Array::default(0),
            labels_set: HashSet::new(),
            cluster_separation: Array::default((0, 0)),
            previous_groups: HashMap::new(),
            exclusive_groups: HashMap::new(),
        }
    }

    /// Runs the clustering engine, linkage engine, and genotype abundances engine
    /// Returns a tuple containing the number of found strains and a `Vec<VariantContext>` with
    /// each context tagged with one or more strains.
    pub fn perform_clustering(
        mut self,
        sample_names: &[String],
        flag_filters: &FlagFilter,
        n_threads: usize,
        tree: &Arc<Mutex<Vec<&Elem>>>,
    ) -> (usize, Vec<VariantContext>) {
        // Creates the depth array used by flight
        let file_name = self.prepare_depth_file();
        {
            let pb = &tree.lock().unwrap()[self.ref_idx + 2];
            pb.progress_bar
                .set_message(format!("{}: Running UMAP and HDBSCAN...", self.ref_name,));
        }
        self.run_flight(file_name);
        // debug!("Flight complete.");
        self.apply_clusters();
        // debug!("Variant groups tagged.");

        // variant groups organized into potential strains
        {
            let pb = &tree.lock().unwrap()[self.ref_idx + 2];
            pb.progress_bar
                .set_message(format!("{}: Linking variant groups...", self.ref_name,));
        }

        // debug!("separation {:?}", &self.cluster_separation);
        let grouped_contexts = self.group_contexts();

        let linkage_engine = LinkageEngine::new(
            grouped_contexts,
            // sample_names,
            &self.cluster_separation,
            &self.previous_groups,
            &self.exclusive_groups,
        );
        let potential_strains = linkage_engine.run_linkage(
            sample_names,
            n_threads,
            &format!("{}/{}", self.output_prefix, self.ref_name),
            flag_filters,
        );
        // debug!("Potential strains {:?}", potential_strains);

        (
            potential_strains.len(),
            self.annotate_variant_contexts_with_strains(potential_strains),
        )
    }

    fn annotate_variant_contexts_with_strains(
        self,
        potential_strains: Vec<LinkedHashSet<i32>>,
    ) -> Vec<VariantContext> {
        // regroup contexts but owned
        let mut grouped_contexts = LinkedHashMap::with_capacity(self.labels_set.len());
        for context in self.variants {
            if let AttributeObject::I32(val) = context
                .attributes
                .get(VariantAnnotations::VariantGroup.to_key())
                .unwrap()
            {
                let group = grouped_contexts.entry(*val).or_insert(Vec::new());
                group.push(context);
            }
        }

        // debug!("Number of groups {}", grouped_contexts.len());

        for (strain_idx, groups_in_strain) in potential_strains.into_iter().enumerate() {
            // debug!(
            //     "Strain index {} groups in strain {:?}",
            //     strain_idx, &groups_in_strain
            // );
            for group in groups_in_strain {
                // debug!("Group {}", group);
                let variant_contexts = grouped_contexts.entry(group).or_insert(Vec::new());
                for vc in variant_contexts {
                    let vc_strain = vc
                        .attributes
                        .entry(VariantAnnotations::Strain.to_key().to_string())
                        .or_insert(AttributeObject::VecUnsize(Vec::new()));
                    if let AttributeObject::VecUnsize(vec) = vc_strain {
                        vec.push(strain_idx)
                    }
                }
            }
        }

        let mut return_contexts = grouped_contexts
            .into_iter()
            .flat_map(|(_, vc_vec)| vc_vec.into_iter())
            .collect::<Vec<VariantContext>>();
        return_contexts.par_sort_unstable();

        return_contexts
    }

    /// Group contexts by their variant group and return a HashMap
    /// key is variant group, value is  vector reference the variant context
    /// The variant context should be sorted by location if they have been generated by
    /// the HaplotypeCallerEngine
    fn group_contexts(&self) -> LinkedHashMap<i32, Vec<&VariantContext>> {
        let mut grouped_contexts = LinkedHashMap::with_capacity(self.labels_set.len());
        for context in self.variants.iter() {
            match context
                .attributes
                .get(VariantAnnotations::VariantGroup.to_key())
            {
                None => continue,
                Some(attribute) => {
                    if let AttributeObject::I32(val) = attribute {
                        if *val != -1 {
                            let group = grouped_contexts.entry(*val).or_insert(Vec::new());
                            group.push(context);
                        }
                    }
                }
            }
        }

        return grouped_contexts;
    }

    fn apply_clusters(&mut self) {
        let max_label = self.labels.iter().max().unwrap();

        // let mut prev_pos = -1;
        // let mut prev_tid = -1;
        // let mut prev_vg = -1;
        let _new_label = *max_label + 1;

        for (idx, vc) in self.variants.iter_mut().enumerate() {
            let variant_group = self.labels[[idx]];
            vc.attributes.insert(
                VariantAnnotations::VariantGroup.to_key().to_string(),
                AttributeObject::I32(variant_group),
            );

            // prev_vg = variant_group;
            // prev_tid = vc.loc.tid();
            // prev_pos = vc.loc.start as i32;
        }
    }

    /// Writes out a variant by sample depth array from the provided collection of variant contexts
    fn prepare_depth_file(&self) -> String {
        // debug!("Writing depth file...");
        let file_name = format!("{}/{}", self.output_prefix, self.ref_name,);
        // ensure path exists
        create_dir_all(self.output_prefix).expect("Unable to create output directory");

        // Depth array for each variant across all samples
        // Each variant (row) is accompanied by n_samples * 2 columns. The columns contain the depth
        // information for the reference and alternate alleles. Thus each sample is represented by two
        // columns. The reference allele always comes first.
        let mut var_depth_array: Array2<i32> =
            Array::from_elem((self.variants.len(), self.n_samples * 2 + 2), 0);

        for (row_id, var) in self.variants.iter().enumerate() {
            var_depth_array[[row_id, 0]] = var.loc.tid();
            var_depth_array[[row_id, 1]] = var.loc.start as i32;
            for (sample_index, genotype) in var.genotypes.genotypes().into_iter().enumerate() {
                for (offset, val) in genotype.ad_i32().iter().enumerate() {
                    if offset < 2 {
                        var_depth_array[[row_id, sample_index * 2 + offset + 2]] = *val
                    }
                }
            }
        }

        write_npy(format!("{}.npy", &file_name), &var_depth_array)
            .expect("Unable to create npy file");

        return file_name;
    }

    fn run_flight<S: AsRef<str>>(&mut self, file_name: S) {
        let cmd_string = format!(
            "flight fit --input {}.npy --cores {}",
            file_name.as_ref(),
            self.allowed_threads,
        );

        // Run the flight command
        finish_command_safely(
            std::process::Command::new("bash")
                .arg("-c")
                .arg(&cmd_string)
                .stderr(std::process::Stdio::piped())
                // .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("Unable to execute bash"),
            "flight",
        );

        // Read in the results
        let labels: Array1<i32> =
            read_npy(format!("{}_labels.npy", file_name.as_ref())).expect("Unable to read npy");
        let labels_set = labels.iter().map(|l| *l).collect::<HashSet<i32>>();

        let cluster_separation: Array2<f64> =
            read_npy(format!("{}_separation.npy", file_name.as_ref())).expect("Unable to read npy");

        self.labels = labels;
        self.labels_set = labels_set;
        self.cluster_separation = cluster_separation;
    }
}
