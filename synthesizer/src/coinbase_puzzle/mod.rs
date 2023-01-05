// Copyright (C) 2019-2022 Aleo Systems Inc.
// This file is part of the snarkVM library.

// The snarkVM library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The snarkVM library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the snarkVM library. If not, see <https://www.gnu.org/licenses/>.

use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

use measure_time::{debug_time, error_time, info_time, print_time, trace_time};
#[cfg(feature = "parallel")]
use rayon::prelude::*;

use console::{
    account::Address,
    prelude::{anyhow, bail, cfg_iter, ensure, has_duplicates, Network, Result, ToBytes},
    program::cfg_into_iter,
};
use hash::*;
pub use helpers::*;
use snarkvm_algorithms::{
    fft::{DensePolynomial, EvaluationDomain},
    msm::VariableBase,
    polycommit::kzg10::{KZG10, KZGCommitment, UniversalParams as SRS},
};
use snarkvm_curves::PairingEngine;
use snarkvm_fields::{PrimeField, Zero};

use crate::UniversalSRS;

mod helpers;
mod hash;
#[cfg(test)]
mod tests;

//use tokio::task::JoinHandle;

#[derive(Clone)]
pub enum CoinbasePuzzle<N: Network> {
    /// The prover contains the coinbase puzzle proving key.
    Prover(Arc<CoinbaseProvingKey<N>>),
    /// The verifier contains the coinbase puzzle verifying key.
    Verifier(Arc<CoinbaseVerifyingKey<N>>),
}

pub fn prove_ex_inner<N: Network>(
    pk: &CoinbaseProvingKey<N>,
    polynomial: &DensePolynomial<<N::PairingCurve as PairingEngine>::Fr>,
    epoch_challenge: &EpochChallenge<N>,
    address: &Address<N>,
    nonce: u64,
    minimum_proof_target: u64,
    product_evaluations: &[<N::PairingCurve as PairingEngine>::Fr],
) -> Result<ProverSolution<N>> {
    let (commitment, _rand) =
        KZG10::commit_lagrange(&pk.lagrange_basis(), &product_evaluations.clone(), None, &Default::default(), None)?;

    let partial_solution = PartialSolution::new(*address, nonce, commitment);

    // Check that the minimum target is met.
    if let minimum_target = minimum_proof_target.clone() {
        let proof_target = partial_solution.to_target()?;
        info!("### proof target ({proof_target} < {minimum_target})");
        ensure!(
                proof_target >= minimum_target,
                "Prover solution was below the necessary proof target ({proof_target} < {minimum_target})"
            );
    }

    info!("### proof target ok ####");


    let point = hash_commitment(&commitment)?;
    let product_eval_at_point = polynomial.evaluate(point) * epoch_challenge.epoch_polynomial().evaluate(point);

    let proof = KZG10::open_lagrange(
        &pk.lagrange_basis(),
        pk.product_domain_elements(),
        &product_evaluations,
        point,
        product_eval_at_point,
    )?;
    ensure!(!proof.is_hiding(), "The prover solution must contain a non-hiding proof");

    debug_assert!(KZG10::check(&pk.verifying_key, &commitment, point, product_eval_at_point, &proof)?);

    Ok(ProverSolution::new(partial_solution, proof))
}


impl<N: Network> CoinbasePuzzle<N> {
    /// Initializes a new `SRS` for the coinbase puzzle.
    #[cfg(any(test, feature = "setup"))]
    pub fn setup(config: PuzzleConfig) -> Result<SRS<N::PairingCurve>> {
        // The SRS must support committing to the product of two degree `n` polynomials.
        // Thus, the SRS must support committing to a polynomial of degree `2n - 1`.
        let total_degree = (2 * config.degree - 1).try_into()?;
        let srs = KZG10::load_srs(total_degree)?;
        Ok(srs)
    }

    /// Load the coinbase puzzle proving and verifying keys.
    pub fn load() -> Result<Self> {
        let max_degree = N::COINBASE_PUZZLE_DEGREE;
        // Load the universal SRS.
        let universal_srs = UniversalSRS::<N>::load()?;
        // Trim the universal SRS to the maximum degree.
        Self::trim(&*universal_srs, PuzzleConfig { degree: max_degree })
    }

    pub fn trim(srs: &SRS<N::PairingCurve>, config: PuzzleConfig) -> Result<Self> {
        // As above, we must support committing to the product of two degree `n` polynomials.
        // Thus, the SRS must support committing to a polynomial of degree `2n - 1`.
        // Since the upper bound to `srs.powers_of_beta_g` takes as input the number
        // of coefficients. The degree of the product has `2n - 1` coefficients.
        //
        // Hence, we request the powers of beta for the interval [0, 2n].
        let product_domain = Self::product_domain(config.degree)?;

        let lagrange_basis_at_beta_g = srs.lagrange_basis(product_domain)?;
        let fft_precomputation = product_domain.precompute_fft();
        let product_domain_elements = product_domain.elements().collect();

        let vk = CoinbaseVerifyingKey::<N> {
            g: srs.power_of_beta_g(0)?,
            gamma_g: <N::PairingCurve as PairingEngine>::G1Affine::zero(), // We don't use gamma_g later on since we are not hiding.
            h: srs.h,
            beta_h: srs.beta_h(),
            prepared_h: srs.prepared_h.clone(),
            prepared_beta_h: srs.prepared_beta_h.clone(),
        };

        let pk = CoinbaseProvingKey {
            product_domain,
            product_domain_elements,
            lagrange_basis_at_beta_g,
            fft_precomputation,
            verifying_key: vk,
        };

        Ok(Self::Prover(Arc::new(pk)))
    }

    /// Returns a prover solution to the coinbase puzzle.
    pub fn prove(
        &self,
        epoch_challenge: &EpochChallenge<N>,
        address: Address<N>,
        nonce: u64,
        minimum_proof_target: Option<u64>,
    ) -> Result<ProverSolution<N>> {
        // Retrieve the coinbase proving key.
        let pk = match self {
            Self::Prover(coinbase_proving_key) => coinbase_proving_key,
            Self::Verifier(_) => bail!("Cannot prove the coinbase puzzle with a verifier"),
        };

        let polynomial = Self::prover_polynomial(epoch_challenge, address, nonce)?;

        let product_evaluations = {
            let polynomial_evaluations = pk.product_domain.in_order_fft_with_pc(&polynomial, &pk.fft_precomputation);


            let product_evaluations = pk.product_domain.mul_polynomials_in_evaluation_domain(
                &polynomial_evaluations,
                &epoch_challenge.epoch_polynomial_evaluations().evaluations,
            );

            product_evaluations
        };


        let (commitment, _rand) =
            KZG10::commit_lagrange(&pk.lagrange_basis(), &product_evaluations, None, &Default::default(), None)?;


        let partial_solution = PartialSolution::new(address, nonce, commitment);

        // Check that the minimum target is met.
        if let Some(minimum_target) = minimum_proof_target {
            let proof_target = partial_solution.to_target()?;
            ensure!(
                proof_target >= minimum_target,
                "Prover solution was below the necessary proof target ({proof_target} < {minimum_target})"
            );
        }

        let point = hash_commitment(&commitment)?;
        let product_eval_at_point = polynomial.evaluate(point) * epoch_challenge.epoch_polynomial().evaluate(point);

        let proof = KZG10::open_lagrange(
            &pk.lagrange_basis(),
            pk.product_domain_elements(),
            &product_evaluations,
            point,
            product_eval_at_point,
        )?;
        ensure!(!proof.is_hiding(), "The prover solution must contain a non-hiding proof");

        debug_assert!(KZG10::check(&pk.verifying_key, &commitment, point, product_eval_at_point, &proof)?);

        Ok(ProverSolution::new(partial_solution, proof))
    }


    pub fn prove_ex(
        &self,
        epoch_challenge: &EpochChallenge<N>,
        address: Address<N>,
        nonce: u64,
        minimum_proof_target: u64,
    ) -> Result<Vec<ProverSolution<N>>> {
        info!("### call prove_ex");

        // Retrieve the coinbase proving key.
        let pk = match self {
            Self::Prover(coinbase_proving_key) => coinbase_proving_key,
            Self::Verifier(_) => bail!("Cannot prove the coinbase puzzle with a verifier"),
        };

        let polynomial = Self::prover_polynomial(epoch_challenge, address, nonce)?;

        //
        // let product_evaluations = {
        //
        //     let polynomial_evaluations = pk.product_domain.in_order_fft_with_pc(&polynomial, &pk.fft_precomputation);
        //
        //
        //     let product_evaluations = pk.product_domain.mul_polynomials_in_evaluation_domain(
        //         &polynomial_evaluations,
        //         &epoch_challenge.epoch_polynomial_evaluations().evaluations,
        //     );
        //
        //     product_evaluations
        // };
        let product_evaluations = {
            let polynomial_evaluations = pk.product_domain.in_order_fft_with_pc(&polynomial0, &pk0.fft_precomputation);

            let product_evaluations = pk.product_domain.mul_polynomials_in_evaluation_domain(
                &polynomial_evaluations,
                &epoch_challenge0.epoch_polynomial_evaluations().evaluations,
            );

            product_evaluations
        };
        // let mut pe_run_flag = true;
        // let (pe_tx, pe_rx) =  crossbeam::channel::bounded(1000);
        // let mut pe_handles = Vec::new();
        // for i in 0..50  {
        //     let pk0 = pk.clone();
        //     let polynomial0 = polynomial.clone();
        //     let epoch_challenge0 = epoch_challenge.clone();
        //     let pe_tx0 = pe_tx.clone();
        //
        //     let handle = std::thread::spawn(move || {
        //         loop {
        //             let now = std::time::Instant::now();
        //             let product_evaluations0 = product_evaluations.clone();
        //             let send_r = pe_tx0.send(product_evaluations0) ;
        //             match send_r {
        //                 Err(e) => {
        //                     info!("### send err : {:?}",e )
        //                 },
        //                 _=>{}
        //             }
        //             info!("### pe_tx0 send ({}ms)",now.elapsed().as_millis() );
        //         }
        //     });
        //     pe_handles.push(handle);
        // }
        // // pe_run_flag = false;
        // info!("### wait pe_handles");
        // for handle in pe_handles {
        //     handle.join().unwrap();
        // }
        // info!("### wait pe_handles ok ");

        let mut rets = Vec::<ProverSolution<N>>::new();

        info!("### begin pe use ");


        // for _i in 0..10 {
        let thread_sizes = 20;
        let mut handles = Vec::with_capacity(thread_sizes);
        for i in 0..thread_sizes {
            let pk0 = pk.clone();
            let polynomial0 = polynomial.clone();
            let epoch_challenge0 = epoch_challenge.clone();
            let nonce0 = nonce.clone();
            let minimum_proof_target0 = minimum_proof_target.clone();
            let address0 = address.clone();

            // let product_evaluations= for pe_handle in pe_handles {
            //     let ret = pe_handle.join().unwrap();
            //     if let Ok(s) = ret {
            //         s
            //     }
            // };

            // let product_evaluations0 = product_evaluations.clone();
            let pe_rx0 = pe_rx.clone();
            let handle = std::thread::spawn(move || {

                loop {
                    info!("### pe_rx recv begin len={}",pe_rx0.len() );
                    let now = std::time::Instant::now();
                    // let product_evaluations0 = pe_rx0.recv().unwrap();

                    info!("### pe_rx recv end ({}ms) len={}",now.elapsed().as_millis() ,pe_rx0.len());

                    info!("### call prove_ex_inner start");
                    let now1 = std::time::Instant::now();
                    let _ = prove_ex_inner(&pk0, &polynomial0, &epoch_challenge0, &address0, nonce0, minimum_proof_target0, &product_evaluations);
                    info!("### call prove_ex_inner end  ({}ms)",now1.elapsed().as_millis());
                    // ret
                }
            });

            handles.push(handle);
        }

        info!("## handles join start");
        for handle in handles {
            // let ret = handle.join().unwrap();
            // if let Ok(s) = ret {
            //     rets.push(s);
            // }
            handle.join().unwrap();
        }
        info!("## handles join end");



        info!("### end pe use ");

        // }
        // pe_run_flag = false;
        // for handle in pe_handles {
        //     handle.join().unwrap();
        //
        // }
        Ok(rets)
    }

    /// Returns a coinbase solution for the given epoch challenge and prover solutions.
    ///
    /// # Note
    /// This method does *not* check that the prover solutions are valid.
    pub fn accumulate_unchecked(
        &self,
        epoch_challenge: &EpochChallenge<N>,
        prover_solutions: &[ProverSolution<N>],
    ) -> Result<CoinbaseSolution<N>> {
        // Ensure there exists prover solutions.
        if prover_solutions.is_empty() {
            bail!("Cannot accumulate an empty list of prover solutions.");
        }

        // Ensure the number of prover solutions does not exceed `MAX_PROVER_SOLUTIONS`.
        if prover_solutions.len() > N::MAX_PROVER_SOLUTIONS {
            bail!(
                "Cannot accumulate beyond {} prover solutions, found {}.",
                prover_solutions.len(),
                N::MAX_PROVER_SOLUTIONS
            );
        }

        // Retrieve the coinbase proving key.
        let pk = match self {
            Self::Prover(coinbase_proving_key) => coinbase_proving_key,
            Self::Verifier(_) => bail!("Cannot accumulate the coinbase puzzle with a verifier"),
        };
        ensure!(!has_duplicates(prover_solutions), "Cannot accumulate duplicate prover solutions");

        let (prover_polynomials, partial_solutions): (Vec<_>, Vec<_>) = cfg_iter!(prover_solutions)
            .filter_map(|solution| {
                if solution.proof().is_hiding() {
                    return None;
                }
                let polynomial = solution.to_prover_polynomial(epoch_challenge).ok()?;
                Some((polynomial, PartialSolution::new(solution.address(), solution.nonce1(), solution.commitment())))
            })
            .unzip();

        // Compute the challenge points.
        let mut challenges = hash_commitments(partial_solutions.iter().map(|solution| *solution.commitment()))?;
        ensure!(challenges.len() == partial_solutions.len() + 1, "Invalid number of challenge points");

        // Pop the last challenge as the accumulator challenge point.
        let accumulator_point = match challenges.pop() {
            Some(point) => point,
            None => bail!("Missing the accumulator challenge point"),
        };

        // Construct the provers polynomial.
        let accumulated_prover_polynomial = cfg_into_iter!(prover_polynomials)
            .zip_eq(challenges)
            .fold(DensePolynomial::zero, |mut accumulator, (mut prover_polynomial, challenge)| {
                prover_polynomial *= challenge;
                accumulator += &prover_polynomial;
                accumulator
            })
            .sum::<DensePolynomial<_>>();
        let product_eval_at_challenge_point = accumulated_prover_polynomial.evaluate(accumulator_point)
            * epoch_challenge.epoch_polynomial().evaluate(accumulator_point);

        // Compute the accumulator polynomial.
        let product_evals = {
            let accumulated_polynomial_evaluations =
                pk.product_domain.in_order_fft_with_pc(&accumulated_prover_polynomial.coeffs, &pk.fft_precomputation);
            pk.product_domain.mul_polynomials_in_evaluation_domain(
                &accumulated_polynomial_evaluations,
                &epoch_challenge.epoch_polynomial_evaluations().evaluations,
            )
        };

        // Compute the coinbase proof.
        let proof = KZG10::open_lagrange(
            &pk.lagrange_basis(),
            pk.product_domain_elements(),
            &product_evals,
            accumulator_point,
            product_eval_at_challenge_point,
        )?;

        // Ensure the coinbase proof is non-hiding.
        if proof.is_hiding() {
            bail!("The coinbase proof must be non-hiding");
        }

        // Return the accumulated proof.
        Ok(CoinbaseSolution::new(partial_solutions, proof))
    }

    /// Returns `true` if the coinbase solution is valid.
    pub fn verify(
        &self,
        coinbase_solution: &CoinbaseSolution<N>,
        epoch_challenge: &EpochChallenge<N>,
        coinbase_target: u64,
        proof_target: u64,
    ) -> Result<bool> {
        // Ensure the coinbase solution is not empty.
        if coinbase_solution.is_empty() {
            bail!("The coinbase solution does not contain any partial solutions");
        }

        // Ensure the number of partial solutions does not exceed `MAX_PROVER_SOLUTIONS`.
        if coinbase_solution.len() > N::MAX_PROVER_SOLUTIONS {
            bail!(
                "The coinbase solution exceeds the allowed number of partial solutions. ({} > {})",
                coinbase_solution.len(),
                N::MAX_PROVER_SOLUTIONS
            );
        }

        // Ensure the coinbase proof is non-hiding.
        if coinbase_solution.proof().is_hiding() {
            bail!("The coinbase proof must be non-hiding");
        }

        // Ensure the coinbase proof meets the required coinbase target.
        if coinbase_solution.to_cumulative_proof_target()? < coinbase_target as u128 {
            bail!("The coinbase proof does not meet the coinbase target");
        }

        // Ensure the puzzle commitments are unique.
        if has_duplicates(coinbase_solution.puzzle_commitments()) {
            bail!("The coinbase solution contains duplicate puzzle commitments");
        }

        // Compute the prover polynomials.
        let prover_polynomials = cfg_iter!(coinbase_solution.partial_solutions())
            // Ensure that each of the prover solutions meets the required proof target.
            .map(|solution| match solution.to_target()? >= proof_target {
                // Compute the prover polynomial.
                true => solution.to_prover_polynomial(epoch_challenge),
                false => bail!("Prover puzzle does not meet the proof target requirements."),
            })
            .collect::<Result<Vec<_>>>()?;

        // Compute the challenge points.
        let mut challenge_points =
            hash_commitments(coinbase_solution.partial_solutions().iter().map(|solution| *solution.commitment()))?;
        ensure!(
            challenge_points.len() == coinbase_solution.partial_solutions().len() + 1,
            "Invalid number of challenge points"
        );

        // Pop the last challenge point as the accumulator challenge point.
        let accumulator_point = match challenge_points.pop() {
            Some(point) => point,
            None => bail!("Missing the accumulator challenge point"),
        };

        // Compute the accumulator evaluation.
        let mut accumulator_evaluation = cfg_iter!(prover_polynomials)
            .zip_eq(&challenge_points)
            .fold(<N::PairingCurve as PairingEngine>::Fr::zero, |accumulator, (prover_polynomial, challenge_point)| {
                accumulator + (prover_polynomial.evaluate(accumulator_point) * challenge_point)
            })
            .sum();
        accumulator_evaluation *= &epoch_challenge.epoch_polynomial().evaluate(accumulator_point);

        // Compute the accumulator commitment.
        let commitments: Vec<_> =
            cfg_iter!(coinbase_solution.partial_solutions()).map(|solution| solution.commitment().0).collect();
        let fs_challenges = challenge_points.into_iter().map(|f| f.to_bigint()).collect::<Vec<_>>();
        let accumulator_commitment =
            KZGCommitment::<N::PairingCurve>(VariableBase::msm(&commitments, &fs_challenges).into());

        // Retrieve the coinbase verifying key.
        let coinbase_verifying_key = match self {
            Self::Prover(coinbase_proving_key) => &coinbase_proving_key.verifying_key,
            Self::Verifier(coinbase_verifying_key) => coinbase_verifying_key,
        };

        // Return the verification result.
        Ok(KZG10::check(
            coinbase_verifying_key,
            &accumulator_commitment,
            accumulator_point,
            accumulator_evaluation,
            coinbase_solution.proof(),
        )?)
    }

    /// Returns the coinbase proving key.
    pub fn coinbase_proving_key(&self) -> Result<&CoinbaseProvingKey<N>> {
        match self {
            Self::Prover(coinbase_proving_key) => Ok(coinbase_proving_key),
            Self::Verifier(_) => bail!("Cannot fetch the coinbase proving key with a verifier"),
        }
    }

    /// Returns the coinbase verifying key.
    pub fn coinbase_verifying_key(&self) -> &CoinbaseVerifyingKey<N> {
        match self {
            Self::Prover(coinbase_proving_key) => &coinbase_proving_key.verifying_key,
            Self::Verifier(coinbase_verifying_key) => coinbase_verifying_key,
        }
    }
}

impl<N: Network> CoinbasePuzzle<N> {
    /// Checks that the degree for the epoch and prover polynomial is within bounds,
    /// and returns the evaluation domain for the product polynomial.
    pub(crate) fn product_domain(degree: u32) -> Result<EvaluationDomain<N::Field>> {
        ensure!(degree != 0, "Degree cannot be zero");
        let num_coefficients = degree.checked_add(1).ok_or_else(|| anyhow!("Degree is too large"))?;
        let product_num_coefficients = num_coefficients
            .checked_mul(2)
            .and_then(|t| t.checked_sub(1))
            .ok_or_else(|| anyhow!("Degree is too large"))?;
        assert_eq!(product_num_coefficients, 2 * degree + 1);
        let product_domain =
            EvaluationDomain::new(product_num_coefficients.try_into()?).ok_or_else(|| anyhow!("Invalid degree"))?;
        assert_eq!(product_domain.size(), (product_num_coefficients as usize).checked_next_power_of_two().unwrap());
        Ok(product_domain)
    }

    /// Returns the prover polynomial for the coinbase puzzle.
    fn prover_polynomial(
        epoch_challenge: &EpochChallenge<N>,
        address: Address<N>,
        nonce: u64,
    ) -> Result<DensePolynomial<<N::PairingCurve as PairingEngine>::Fr>> {
        info_time!("prover_polynomial function");
        {
            let input = {
                let mut bytes = [0u8; 76];
                bytes[..4].copy_from_slice(&epoch_challenge.epoch_number().to_bytes_le()?);
                bytes[4..36].copy_from_slice(&epoch_challenge.epoch_block_hash().to_bytes_le()?);
                bytes[36..68].copy_from_slice(&address.to_bytes_le()?);
                bytes[68..].copy_from_slice(&nonce.to_le_bytes());
                bytes
            };
            Ok(hash_to_polynomial::<<N::PairingCurve as PairingEngine>::Fr>(&input, epoch_challenge.degree()))
        }
    }
}
