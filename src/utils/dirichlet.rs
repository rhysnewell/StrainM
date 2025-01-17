use statrs::function::gamma;

use crate::utils::math_utils::LOG10_E;

pub struct Dirichlet<'a> {
    alpha: &'a [f64],
}

impl<'a> Dirichlet<'a> {
    pub fn new(alpha: &'a [f64]) -> Dirichlet<'a> {
        Dirichlet { alpha }
    }

    // in variational Bayes one often needs the effective point estimate of a multinomial distribution with a
    // Dirichlet prior.  This value is not the mode or mean of the Dirichlet but rather the exp of the expected log weights.
    // note that these effective weights do not add up to 1.  This is fine because in any probabilistic model scaling all weights
    // amounts to an arbitrary normalization constant, but it's important to keep in mind because some classes may expect
    // normalized weights.  In that case the calling code must normalize the weights.
    pub fn effective_multinomial_weights(&self) -> Vec<f64> {
        let digamma_of_sum = gamma::digamma(self.alpha.iter().sum::<f64>());
        let result = self
            .alpha
            .iter()
            .map(|a| (gamma::digamma(*a) - digamma_of_sum).exp())
            .collect::<Vec<f64>>();

        return result;
    }

    pub fn effective_log10_multinomial_weights(&self) -> Vec<f64> {
        let digamma_of_sum = gamma::digamma(self.alpha.iter().sum::<f64>());
        let result = self
            .alpha
            .iter()
            .map(|a| (gamma::digamma(*a) - digamma_of_sum) * *LOG10_E)
            .collect::<Vec<f64>>();

        return result;
    }

    pub fn effective_log_multinomial_weights(&self) -> Vec<f64> {
        let digamma_of_sum = gamma::digamma(self.alpha.iter().sum::<f64>());
        let result = self
            .alpha
            .iter()
            .map(|a| gamma::digamma(*a) - digamma_of_sum)
            .collect::<Vec<f64>>();

        return result;
    }

    pub fn mean_weights(&self) -> Vec<f64> {
        let sum = self.alpha.iter().sum::<f64>();
        let result = self.alpha.iter().map(|x| *x / sum).collect::<Vec<f64>>();

        return result;
    }

    pub fn log10_mean_weights(&self) -> Vec<f64> {
        let sum = self.alpha.iter().sum::<f64>();
        let result = self
            .alpha
            .iter()
            .map(|x| (*x / sum).log10())
            .collect::<Vec<f64>>();

        return result;
    }

    pub fn size(&self) -> usize {
        self.alpha.len()
    }
}
