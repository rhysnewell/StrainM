use ndarray::{Array2, Array1, Axis, ArrayView, Ix1};
use ndarray_linalg::{SVD, convert::*, diagonal::*, Norm};
use rayon::prelude::*;
use std::sync::{Arc, Mutex};
use std::process;

#[derive(Debug, Clone, Copy)]
pub enum Seed {
    Nndsvd {
        rank: usize,
    },
    None,
}

impl Seed {
    pub fn new_nndsvd(rank: usize, v: &Array2<f32>) -> Seed {
        Seed::Nndsvd {
            rank,
        }
    }
}

pub trait SeedFunctions {
    fn initialize(&self, v: &Array2<f32>) -> (Array2<f32>, Array2<f32>);
}

impl SeedFunctions for Seed {
    fn initialize(&self, v: &Array2<f32>) -> (Array2<f32>, Array2<f32>) {
        match self {
            Seed::Nndsvd {
                rank,
            } => {
                let (u, s, e)
                    = v.svd(true, true).unwrap();
                let e = e.unwrap();
                let e = e.t();
                let u = u.unwrap();

                let mut w = Array2::zeros((v.shape()[0], *rank));
                let mut h = Array2::zeros((*rank, v.shape()[1]));

                // choose the first singular triplet to be nonnegative
                let s = s.into_diag();
                debug!("S: {:?}", s);
                w.slice_mut(s![.., 0]).assign(
                    &(s[0].powf(1. / 2.) * u.slice(s![.., 0]).mapv(|x| x.abs())));
                h.slice_mut(s![0, ..]).assign(
                    &(s[0].powf(1. / 2.) * e.slice(s![.., 0]).t().mapv(|x| x.abs())));

                // generate mutex guards around w and h
                let w_guard = Arc::new(Mutex::new(w.clone()));
                let h_guard = Arc::new(Mutex::new(h.clone()));

                // second svd for the other factors
                (1..*rank).into_par_iter().for_each(|i|{
                    let uu = u.slice(s![.., i]);
                    let vv = e.slice(s![.., i]);
                    let uup = pos(&uu);
                    let uun = neg(&uu);
                    let vvp = pos(&vv);
                    let vvn = neg(&vv);
                    let n_uup = uup.norm();
                    let n_uun = uun.norm();
                    let n_vvp = vvp.norm();
                    let n_vvn = vvn.norm();
                    let termp = n_uup * n_vvp;
                    let termn = n_uun * n_vvn;

                    if termp >= termn {
                        let mut w_guard = w_guard.lock().unwrap();
                        let mut h_guard = h_guard.lock().unwrap();
                        w_guard.slice_mut(s![.., i]).assign(
                            &((s[i] * termp).powf(1. / 2.) / (uup.mapv(|x| x * n_uup))));
                        h_guard.slice_mut(s![i, ..]).assign(
                            &((s[i] * termp).powf(1. / 2.) / (vvp.t().mapv(|x| x * n_vvp))));;
                    } else {
                        let mut w_guard = w_guard.lock().unwrap();
                        let mut h_guard = h_guard.lock().unwrap();
                        w_guard.slice_mut(s![.., i]).assign(
                            &((s[i] * termn).powf(1. / 2.) / (uun.mapv(|x| x * n_uun))));
                        h_guard.slice_mut(s![i, ..]).assign(
                            &((s[i] * termn).powf(1. / 2.) / (vvn.t().mapv(|x| x * n_vvn))));;
                    }
                });
                let w_guard = w_guard.lock().unwrap();
                let h_guard = h_guard.lock().unwrap();


                let w = w_guard.mapv(|x|{
                    if x < 1f32.exp().powf(-11.) {
                        0.
                    } else {
                        x
                    }
                });

                let h = h_guard.mapv(|x|{
                    if x < 1f32.exp().powf(-11.) {
                        0.
                    } else {
                        x
                    }
                });

                debug!("Threshold {}", 1f32.exp().powf(-11.));
                return (w, h)

            },
            Seed::None => process::exit(1)
        }
    }
}

fn pos(matrix: &ArrayView<f32, Ix1>) -> Array1<f32> {
    matrix.mapv(|x| {
        if x >= 0. {
            1.
        } else {
            0.
        }
    }) * matrix
}

fn neg(matrix: &ArrayView<f32, Ix1>) -> Array1<f32> {
    matrix.mapv(|x| {
        if x < 0. {
            1.
        } else {
            0.
        }
    }) * matrix.mapv(|x| {
        if x != 0. {
            -x
        } else {
            x
        }
    })
}