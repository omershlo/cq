use ark_ec::{PairingEngine, AffineCurve};
use ark_ff::Field;
use ark_poly::{GeneralEvaluationDomain, EvaluationDomain, univariate::DensePolynomial, UVPolynomial};

use crate::{table::Table, utils::{is_pow_2, construct_lagrange_basis}, kzg::Kzg, tools::compute_qs};

pub struct Index<E: PairingEngine> {
    pub(crate) zv_2: E::G2Affine, 
    pub(crate) t_2: E::G2Affine,
    pub(crate) qs: Vec<E::G1Affine>,
    pub(crate) ls: Vec<E::G1Affine>, 
    pub(crate) ls_at_0: Vec<E::G1Affine>
}

impl<E: PairingEngine> Index<E> {
    pub fn gen(
        srs_g1: &[E::G1Affine], 
        srs_g2: &[E::G2Affine],
        table: &Table<E::Fr>
    ) -> Self {
        assert!(is_pow_2(table.size));
        let domain = GeneralEvaluationDomain::<E::Fr>::new(table.size).unwrap();
        let n = domain.size(); // same as table.size

        // step 2: compute [zV(x)]_2
        let tau_pow_n = srs_g2[n];
        let minus_one = -E::G2Affine::prime_subgroup_generator();
        let zv_2 = tau_pow_n + minus_one;

        // step 3: compute [T(x)]_2
        let table_poly = DensePolynomial::from_coefficients_slice(&domain.ifft(&table.values));
        let t_2: E::G2Affine = Kzg::<E>::commit_g2(&srs_g2, &table_poly).into();

        // step 4: compute [Qi(x)]_1
        let qs = compute_qs::<E>(&table_poly, &domain, srs_g1);

        // step 5: compute [Li(x)]_1
        // TODO: this should be done in NlogN from https://eprint.iacr.org/2017/602.pdf(3.3)
        let roots: Vec<_> = domain.elements().collect();
        let lagrange_basis = construct_lagrange_basis(&roots);
        let lagrange_basis_1: Vec<E::G1Affine> = lagrange_basis.iter().map(|li| Kzg::<E>::commit_g1(&srs_g1, li).into()).collect();

        // step 6: compute [(Li(x) - Li(0)) / x]_1
        // commit to all zero openings of lagrange basis
        let rhs = srs_g1[n - 1].mul(-domain.size_as_field_element().inverse().unwrap());
        let mut li_proofs: Vec<E::G1Affine> = Vec::with_capacity(n);
        for (i, li_1) in lagrange_basis_1.iter().enumerate() {
            let lhs = li_1.mul(domain.element(n - i));
            li_proofs.push(
                (lhs + rhs).into()
            );
        }

        Self {
            zv_2,
            t_2,
            qs,
            ls: lagrange_basis_1,
            ls_at_0: li_proofs,
        }
    }
}

#[cfg(test)]
mod indexer_tests {
    use ark_bn254::{Bn254, Fr, G2Affine, G1Affine};
    use ark_ec::AffineCurve;
    use ark_ff::{batch_inversion, Field, Zero, UniformRand};
    use ark_poly::{GeneralEvaluationDomain, EvaluationDomain, univariate::DensePolynomial};
    use ark_std::{test_rng, rand::rngs::StdRng};

    use crate::{utils::{unsafe_setup_from_rng, construct_lagrange_basis}, kzg::Kzg, table::Table};

    use super::Index;

    #[test]
    fn test_index_gen() {
        let n = 32; 
        let mut rng = test_rng();

        let (srs_g1, srs_g2) = unsafe_setup_from_rng::<Bn254, StdRng>(n - 1, n, &mut rng);

        let table_values: Vec<_> = (0..n).map(|_| Fr::rand(&mut rng)).collect();
        let table = Table {
            size: n,
            values: table_values,
        };

        let _ = Index::<Bn254>::gen(&srs_g1, &srs_g2, &table);
    }

    #[test]
    fn test_commitments_to_li_at_zero() {
        let n = 32; 
        let mut rng = test_rng();

        let domain = GeneralEvaluationDomain::<Fr>::new(n).unwrap();

        let roots: Vec<_> = domain.elements().collect();
        let lagrange_basis = construct_lagrange_basis(&roots);

        let (srs_g1, _) = unsafe_setup_from_rng::<Bn254, StdRng>(n - 1, 0, &mut rng);

        let lagrange_basis_1: Vec<G1Affine> = lagrange_basis.iter().map(|li| Kzg::<Bn254>::commit_g1(&srs_g1, li).into()).collect();

        let zero = Fr::zero();
        let li_proofs_slow: Vec<G1Affine> = lagrange_basis.iter().map(|li| Kzg::<Bn254>::open_g1(&srs_g1, li, zero).1.into()).collect();

        let rhs = srs_g1[n - 1].mul(-domain.size_as_field_element().inverse().unwrap());
        let mut li_proofs_fast: Vec<G1Affine> = Vec::with_capacity(n);

        for (i, li_1) in lagrange_basis_1.iter().enumerate() {
            let lhs = li_1.mul(domain.element(n - i));
            li_proofs_fast.push(
                (lhs + rhs).into()
            );
        }

        assert_eq!(li_proofs_slow, li_proofs_fast);
    }

    #[test]
    fn test_omega_inverse() {
        let n = 32; 

        let domain = GeneralEvaluationDomain::<Fr>::new(n).unwrap();
        let mut roots_inv: Vec<_> = domain.elements().collect();
        batch_inversion(&mut roots_inv);

        for (i, &omega_i_inv) in roots_inv.iter().enumerate() {
            // w^i * w^(n-i) = 1
            let omega_n_minus_i = domain.element(n - i);
            assert_eq!(omega_i_inv, omega_n_minus_i);
        }
    }

    #[test]
    fn commitment_to_zv() {
        let n = 32; 
        let mut rng = test_rng();

        let domain = GeneralEvaluationDomain::<Fr>::new(n).unwrap();

        let (_, srs_g2) = unsafe_setup_from_rng::<Bn254, StdRng>(0, n, &mut rng);

        let zv: DensePolynomial<_> = domain.vanishing_polynomial().into();
        let full_cm: G2Affine = Kzg::<Bn254>::commit_g2(&srs_g2, &zv).into();

        let tau_pow_n = srs_g2[n];
        let minus_one = -G2Affine::prime_subgroup_generator();
        let cm = tau_pow_n + minus_one;
        assert_eq!(full_cm, cm);
    }
}