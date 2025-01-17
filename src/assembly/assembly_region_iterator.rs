use ordered_float::OrderedFloat;
use rayon::prelude::*;
use std::cmp::Reverse;
use rust_htslib::bam::Record;

use crate::processing::lorikeet_engine::ReadType;
use crate::reads::bird_tool_reads::BirdToolRead;
use crate::reads::read_utils::ReadUtils;
use crate::utils::interval_utils::IntervalUtils;
use crate::utils::simple_interval::SimpleInterval;
use crate::assembly::assembly_region::AssemblyRegion;
use crate::bam_parsing::{
    FlagFilter, 
    bam_generator::{
        generate_indexed_named_bam_readers_from_bam_files, IndexedNamedBamReader,
    }
};

/**
 * Given a {@link BandPassActivityProfile} and {@link AssemblyRegionWalker}, iterates over each {@link AssemblyRegion} within
 * that shard, using the provided {@link AssemblyRegionEvaluator} to determine the boundaries between assembly
 * regions.
 *
 * Loads the reads from the shard as lazily as possible to minimize memory usage.
 *
 * This iterator represents the core of the {@link AssemblyRegionWalker} traversal.
 *
 * NOTE: the provided shard must have appropriate read filters set on it for this traversal type (ie., unmapped
 * and malformed reads must be filtered out).
 * Re-implementation of the GATK code base. Original author unknown
 * Rust implementation:
 * @author Rhys Newell <rhys.newell@hdr.qut.edu.au>
 */
#[derive(Debug)]
pub struct AssemblyRegionIterator<'a> {
    indexed_bam_readers: &'a [String],
    // n_threads: u32,
    // previous_regions_reads: Vec<BirdToolRead>,
}

impl<'a> AssemblyRegionIterator<'a> {
    const DUMMY_LIMITING_INTERVAL: Option<SimpleInterval> = None;

    pub fn new(indexed_bam_readers: &'a [String], _n_threads: u32) -> AssemblyRegionIterator<'a> {
        // Assume no forced conversion here since we have already traverse the entire
        // activity profile prior to reaching here. This is quite different to how
        // GATK handles it but I assume it ends up working the same?
        AssemblyRegionIterator {
            indexed_bam_readers,
            // n_threads,
        }
    }

    pub fn fill_next_assembly_region_with_reads(
        &self,
        region: &mut AssemblyRegion,
        flag_filters: &FlagFilter,
        n_threads: u32,
        short_read_bam_count: usize,
        _long_read_bam_count: usize,
        max_input_depth: usize,
        args: &clap::ArgMatches,
    ) {
        // We don't need to check previous region reads as the implementation of fetch we have
        // should retrieve all reads regardless of if they have been seen before

        let min_mapq = *args.get_one::<u8>("min-mapq").unwrap();
        let min_long_read_size = *args
            .get_one::<usize>("min-long-read-size")
            .unwrap();
        let min_long_read_average_base_qual = *args
            .get_one::<usize>("min-long-read-average-base-qual")
            .unwrap();

        let _limiting_interval = IntervalUtils::parse_limiting_interval(args);

        let mut records: Vec<BirdToolRead> = self
            .indexed_bam_readers
            .par_iter()
            .enumerate()
            .flat_map(|(sample_idx, bam_generator)| {
                let read_type = if sample_idx < short_read_bam_count {
                    ReadType::Short
                } else {
                    ReadType::Long
                };

                match read_type {
                    ReadType::Short | ReadType::Long => {
                        let mut record = Record::new(); // Empty bam record
                        let mut bam_generated = generate_indexed_named_bam_readers_from_bam_files(
                            vec![&bam_generator],
                            n_threads,
                        )
                        .into_iter()
                        .next()
                        .unwrap();
                        // debug!(
                        //     "samples: {} -> {}: {} - {}",
                        //     bam_generator,
                        //     region.get_contig(),
                        //     region.get_padded_span().start,
                        //     region.get_padded_span().end
                        // );
                        bam_generated.fetch((
                            region.get_contig() as i32,
                            region.get_padded_span().start as i64,
                            region.get_padded_span().end as i64 + 1,
                        )).expect("Failed to fetch reads");

                        let mut records = Vec::new(); // container for the records to be collected

                        while bam_generated.read(&mut record) == true {
                            if ReadUtils::read_is_filtered(
                                &record,
                                flag_filters,
                                min_mapq,
                                read_type,
                                &Self::DUMMY_LIMITING_INTERVAL,
                                min_long_read_size,
                                min_long_read_average_base_qual,
                            )
                            // Check against filter flags and current sample type
                            {
                                continue;
                            } else {
                                records.push(BirdToolRead::new(
                                    record.clone(),
                                    sample_idx,
                                    read_type,
                                ));
                            };
                        }

                        records
                    }
                }
            })
            .collect::<Vec<BirdToolRead>>();

        if records.len() > max_input_depth {
            // sort the reads by decreasing mean base quality. Take the top n.
            records.par_sort_by_key(|r| {
                Reverse(OrderedFloat(
                    r.read.qual().iter().map(|bq| *bq as f64).sum::<f64>()
                        / r.read.qual().len() as f64,
                ))
            });
            records = records.into_iter().take(max_input_depth).collect();
        }

        // debug!(
        //     "Region {:?} No. reads {}",
        //     &region.padded_span,
        //     records.len()
        // );
        records.par_sort_unstable();
        region.add_all(records);
    }
}

// impl Iterator for AssemblyRegionIterator<'_> {
//     type Item = AssemblyRegion;
//
//     fn next(&mut self) -> Option<Self::Item> {
//         Some(self.pending_regions.next())
//     }
// }
