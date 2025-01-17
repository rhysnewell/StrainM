use rayon::prelude::*;
use std::collections::HashMap;

/// Implementation of the EM algorithm used by centrifuge
///
/// @author Rhys Newell <rhys.newell@.hdr.qut.edu.au>
#[derive(Debug, Clone)]
pub struct StrainAbundanceCalculator {
    // The genotype index
    pub index: usize,
    // The expected weights of each variant that decide the strain abundance
    pub variant_weights: Vec<f64>,
    // The indices of all the genotypes that share this variant
    pub variant_genotype_ids: Vec<Vec<usize>>,
    // The geometric weighting of this strains abundance
    // Strain abundance would be calculated as abundance_weight * total_abundance
    // denoted as `theta`
    pub abundance_weight: f64,
    // HashMap containing the variant index in all context mapped to the variant index
    // in this calculator
    pub variant_index_map: HashMap<usize, usize>,
    // Reverse of variant index map
    pub index_variant_map: HashMap<usize, usize>,
}

impl StrainAbundanceCalculator {
    pub fn new(index: usize, capacity: usize) -> Self {
        Self {
            index,
            variant_weights: Vec::with_capacity(capacity),
            variant_genotype_ids: Vec::with_capacity(capacity),
            abundance_weight: 1.,
            variant_index_map: HashMap::with_capacity(capacity),
            index_variant_map: HashMap::with_capacity(capacity),
        }
    }

    pub fn calculate_abundances(sample_genotypes: &mut Vec<Self>, eps: f64) {
        // the difference between theta curr and theta prev
        let mut omega = 1.;

        let mut theta_prev;
        let mut theta_curr = vec![1.; sample_genotypes.len()];
        // let mut n = 0;

        while omega > eps {
            // Update theta values
            theta_prev = theta_curr.clone();

            let mut new_weights = Vec::with_capacity(sample_genotypes.len());
            let mut updated_thetas = Vec::with_capacity(sample_genotypes.len());

            for index in 0..sample_genotypes.len() {
                if (sample_genotypes[index].abundance_weight - eps).abs() <= f64::EPSILON
                    || sample_genotypes[index].abundance_weight.is_infinite()
                {
                    updated_thetas.push(0.0);
                    new_weights.push(None);
                    continue;
                }

                // Step 1: update variant weights
                // sample_genotypes[index].variant_weights
                let variant_weights: Vec<f64> = sample_genotypes[index]
                    .variant_weights
                    .par_iter()
                    .enumerate()
                    .map(|(variant_index, w)| {
                        // debug!(
                        //     "All weights for {}: {:?} -> {:?}",
                        //     variant_index,
                        //     &sample_genotypes[index].variant_genotype_ids[variant_index],
                        //     sample_genotypes[index].variant_genotype_ids[variant_index]
                        //         .iter()
                        //         .map(|genotype_index| theta_curr[*genotype_index])
                        //         .collect::<Vec<f64>>()
                        // );
                        let mut pooled_weights = sample_genotypes[index].variant_genotype_ids
                            [variant_index]
                            .iter()
                            .map(|genotype_index| theta_curr[*genotype_index])
                            .sum::<f64>();

                        if pooled_weights <= f64::EPSILON {
                            pooled_weights = 1.0;
                        }
                        // debug!(
                        //     "Variant index {} weight {} pooled weight {} theta curr {} {}",
                        //     variant_index,
                        //     w,
                        //     pooled_weights,
                        //     theta_curr[index],
                        //     theta_curr[index] / pooled_weights
                        // );
                        // if variant weights are between 0 and 1
                        let w = (w * sample_genotypes[index].abundance_weight) / (pooled_weights);
                        w
                    })
                    .collect();

                // Step 2: update abundance weight based on mean of variant weights
                let denominator = sample_genotypes
                    .par_iter()
                    .map(|other_genotypes| other_genotypes.variant_weights.iter().sum::<f64>())
                    .sum::<f64>();
                // sample_genotypes[index].abundance_weight
                let mut abundance_weight = (variant_weights.iter().sum::<f64>()) / denominator;

                // debug!(
                //     "Index {} abundance weight {} variant weights",
                //     index,
                //     abundance_weight, //&genotype.variant_weights
                // );

                if abundance_weight.is_nan()
                    // || genotype.variant_weights.contains(&0.0)
                    || abundance_weight.is_infinite()
                    || abundance_weight < eps
                {
                    abundance_weight = 0.;
                    // sample_genotypes[index].variant_weights = vec![];
                }

                // Update list of theta_curr
                // theta_curr[index] = sample_genotypes[index].abundance_weight;
                updated_thetas.push(abundance_weight);
                new_weights.push(Some(variant_weights));
            }
            for (index, weights) in new_weights.into_iter().enumerate() {
                match weights {
                    None => {
                        // pass
                    }
                    Some(weights) => {
                        sample_genotypes[index].variant_weights = weights;
                        sample_genotypes[index].abundance_weight = updated_thetas[index];
                        theta_curr[index] = updated_thetas[index];
                    }
                }
            }

            // Update omega
            omega = theta_curr
                .iter()
                .zip(theta_prev.iter())
                .map(|(curr, prev)| (curr - prev).abs())
                .sum::<f64>();

            // debug!(
            //     "Theta Current {:?} Prev {:?} Omega {}",
            //     &theta_curr, &theta_prev, &omega,
            // );
            // n += 1;
        }
        // debug!("EM Algorithm Finished in {} iterations", n);
    }
}
