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

use super::*;

impl<A: Aleo> Request<A> {
    /// Returns `true` if the input IDs are derived correctly, the input records all belong to the caller,
    /// and the signature is valid.
    ///
    /// Verifies (challenge == challenge') && (address == address') && (serial_numbers == serial_numbers') where:
    ///     challenge' := HashToScalar(r * G, pk_sig, pr_sig, caller, \[tvk, tcm, function ID, input IDs\])
    pub fn verify(&self, input_types: &[console::ValueType<A::Network>], tpk: &Group<A>) -> Boolean<A> {
        // Compute the function ID as `Hash(network_id, program_id, function_name)`.
        let function_id = A::hash_bhp1024(
            &(&self.network_id, self.program_id.name(), self.program_id.network(), &self.function_name).to_bits_le(),
        );

        // Construct the signature message as `[tvk, tcm, function ID, input IDs]`.
        let mut message = Vec::with_capacity(3 + 4 * self.input_ids.len());
        message.push(self.tvk.clone());
        message.push(self.tcm.clone());
        message.push(function_id);

        // Check the input IDs and construct the rest of the signature message.
        let (input_checks, append_to_message) = Self::check_input_ids::<true>(
            &self.network_id,
            &self.program_id,
            &self.function_name,
            &self.input_ids,
            &self.inputs,
            input_types,
            &self.caller,
            &self.sk_tag,
            &self.tvk,
            &self.tcm,
            Some(&self.signature),
        );
        // Append the input elements to the message.
        match append_to_message {
            Some(append_to_message) => message.extend(append_to_message),
            None => A::halt("Missing input elements in request verification"),
        }

        // Verify the transition public key and transition view key are well-formed.
        let tvk_checks = {
            // Compute the transition public key `tpk` as `tsk * G`.
            let candidate_tpk = A::g_scalar_multiply(&self.tsk);
            // Compute the transition view key `tvk` as `tsk * caller`.
            let tvk = (self.caller.to_group() * &self.tsk).to_x_coordinate();
            // Compute the transition commitment as `Hash(tvk)`.
            let tcm = A::hash_psd2(&[tvk.clone()]);

            // Ensure the computed transition public key matches the expected transition public key.
            tpk.is_equal(&candidate_tpk)
                // Ensure the transition public key matches with the derived one from the signature.
                & tpk.is_equal(&self.to_tpk())
                // Ensure the computed transition view key matches.
                & tvk.is_equal(&self.tvk)
                // Ensure the computed transition commitment matches.
                & tcm.is_equal(&self.tcm)
        };

        // Verify the signature.
        // Note: We copy/paste the Aleo signature verification code here in order to compute `tpk` only once.
        let signature_checks = {
            // Retrieve pk_sig.
            let pk_sig = self.signature.compute_key().pk_sig();
            // Retrieve pr_sig.
            let pr_sig = self.signature.compute_key().pr_sig();

            // Construct the hash input as (r * G, pk_sig, pr_sig, address, message).
            let mut preimage = Vec::with_capacity(4 + message.len());
            preimage.extend([tpk, pk_sig, pr_sig].map(|point| point.to_x_coordinate()));
            preimage.push(self.caller.to_field());
            preimage.extend_from_slice(&message);

            // Compute the candidate verifier challenge.
            let candidate_challenge = A::hash_to_scalar_psd8(&preimage);
            // Compute the candidate address.
            let candidate_address = self.signature.compute_key().to_address();

            // Return `true` if the challenge and address is valid.
            self.signature.challenge().is_equal(&candidate_challenge) & self.caller.is_equal(&candidate_address)
        };

        // Verify the signature, inputs, and `tvk` are valid.
        signature_checks & input_checks & tvk_checks
    }

    /// Returns `true` if the inputs match their input IDs.
    /// Note: This method does **not** perform signature checks.
    pub fn check_input_ids<const CREATE_MESSAGE: bool>(
        network_id: &U16<A>,
        program_id: &ProgramID<A>,
        function_name: &Identifier<A>,
        input_ids: &[InputID<A>],
        inputs: &[Value<A>],
        input_types: &[console::ValueType<A::Network>],
        caller: &Address<A>,
        sk_tag: &Field<A>,
        tvk: &Field<A>,
        tcm: &Field<A>,
        signature: Option<&Signature<A>>,
    ) -> (Boolean<A>, Option<Vec<Field<A>>>) {
        // Ensure the signature response matches the `CREATE_MESSAGE` flag.
        match CREATE_MESSAGE {
            true => assert!(signature.is_some()),
            false => assert!(signature.is_none()),
        }

        // Compute the function ID as `Hash(network_id, program_id, function_name)`.
        let function_id =
            A::hash_bhp1024(&(network_id, program_id.name(), program_id.network(), function_name).to_bits_le());

        // Initialize a vector for a message.
        let mut message = Vec::new();

        // Perform the input ID checks.
        let input_checks = input_ids
            .iter()
            .zip_eq(inputs)
            .zip_eq(input_types)
            .enumerate()
            .map(|(index, ((input_id, input), input_type))| {
                match input_id {
                    // A constant input is hashed (using `tcm`) to a field element.
                    InputID::Constant(input_hash) => {
                        // Add the input hash to the message.
                        if CREATE_MESSAGE {
                            message.push(input_hash.clone());
                        }

                        // Prepare the index as a constant field element.
                        let input_index = Field::constant(console::Field::from_u16(index as u16));
                        // Construct the preimage as `(function ID || input || tcm || index)`.
                        let mut preimage = vec![function_id.clone()];
                        preimage.extend(input.to_fields());
                        preimage.push(tcm.clone());
                        preimage.push(input_index);

                        // Ensure the expected hash matches the computed hash.
                        match &input {
                            Value::Plaintext(..) => input_hash.is_equal(&A::hash_psd8(&preimage)),
                            // Ensure the input is not a record.
                            Value::Record(..) => A::halt("Expected a constant plaintext input, found a record input"),
                        }
                    }
                    // A public input is hashed (using `tcm`) to a field element.
                    InputID::Public(input_hash) => {
                        // Add the input hash to the message.
                        if CREATE_MESSAGE {
                            message.push(input_hash.clone());
                        }

                        // Prepare the index as a constant field element.
                        let input_index = Field::constant(console::Field::from_u16(index as u16));
                        // Construct the preimage as `(function ID || input || tcm || index)`.
                        let mut preimage = vec![function_id.clone()];
                        preimage.extend(input.to_fields());
                        preimage.push(tcm.clone());
                        preimage.push(input_index);

                        // Ensure the expected hash matches the computed hash.
                        match &input {
                            Value::Plaintext(..) => input_hash.is_equal(&A::hash_psd8(&preimage)),
                            // Ensure the input is not a record.
                            Value::Record(..) => A::halt("Expected a public plaintext input, found a record input"),
                        }
                    }
                    // A private input is encrypted (using `tvk`) and hashed to a field element.
                    InputID::Private(input_hash) => {
                        // Add the input hash to the message.
                        if CREATE_MESSAGE {
                            message.push(input_hash.clone());
                        }

                        // Prepare the index as a constant field element.
                        let input_index = Field::constant(console::Field::from_u16(index as u16));
                        // Compute the input view key as `Hash(function ID || tvk || index)`.
                        let input_view_key = A::hash_psd4(&[function_id.clone(), tvk.clone(), input_index]);
                        // Compute the ciphertext.
                        let ciphertext = match &input {
                            Value::Plaintext(plaintext) => plaintext.encrypt_symmetric(input_view_key),
                            // Ensure the input is a plaintext.
                            Value::Record(..) => A::halt("Expected a private plaintext input, found a record input"),
                        };

                        // Ensure the expected hash matches the computed hash.
                        input_hash.is_equal(&A::hash_psd8(&ciphertext.to_fields()))
                    }
                    // A record input is computed to its serial number.
                    InputID::Record(commitment, gamma, serial_number, tag) => {
                        // Retrieve the record.
                        let record = match &input {
                            Value::Record(record) => record,
                            // Ensure the input is a record.
                            Value::Plaintext(..) => A::halt("Expected a record input, found a plaintext input"),
                        };
                        // Retrieve the record name as a `Mode::Constant`.
                        let record_name = match input_type {
                            console::ValueType::Record(record_name) => Identifier::constant(*record_name),
                            // Ensure the input is a record.
                            _ => A::halt(format!("Expected a record input at input {index}")),
                        };
                        // Compute the record commitment.
                        let candidate_commitment = record.to_commitment(program_id, &record_name);
                        // Compute the `candidate_serial_number` from `gamma`.
                        let candidate_serial_number = Record::<A, Plaintext<A>>::serial_number_from_gamma(gamma, candidate_commitment.clone());
                        // Compute the tag.
                        let candidate_tag = Record::<A, Plaintext<A>>::tag(sk_tag.clone(), candidate_commitment.clone());

                        if CREATE_MESSAGE {
                            // Ensure the signature is declared.
                            let signature = match signature {
                                Some(signature) => signature,
                                None => A::halt("Missing signature in logic to check input IDs")
                            };
                            // Retrieve the challenge from the signature.
                            let challenge = signature.challenge();
                            // Retrieve the response from the signature.
                            let response = signature.response();

                            // Compute the generator `H` as `HashToGroup(commitment)`.
                            let h = A::hash_to_group_psd2(&[A::serial_number_domain(), candidate_commitment.clone()]);
                            // Compute `h_r` as `(challenge * gamma) + (response * H)`, equivalent to `r * H`.
                            let h_r = (gamma.deref() * challenge) + (&h * response);

                            // Add (`H`, `r * H`, `gamma`, `tag`) to the message.
                            message.extend([h, h_r, *gamma.clone()].iter().map(|point| point.to_x_coordinate()));
                            message.push(candidate_tag.clone());
                        }

                        // Ensure the candidate serial number matches the expected serial number.
                        serial_number.is_equal(&candidate_serial_number)
                            // Ensure the candidate commitment matches the expected commitment.
                            & commitment.is_equal(&candidate_commitment)
                            // Ensure the candidate tag matches the expected tag.
                            & tag.is_equal(&candidate_tag)
                            // Ensure the record belongs to the caller.
                            & record.owner().deref().is_equal(caller)
                            // Ensure the record gates is less than or equal to 2^52.
                            & !(**record.gates()).to_bits_le()[52..].iter().fold(Boolean::constant(false), |acc, bit| acc | bit)
                    }
                    // An external record input is hashed (using `tvk`) to a field element.
                    InputID::ExternalRecord(input_hash) => {
                        // Add the input hash to the message.
                        if CREATE_MESSAGE {
                            message.push(input_hash.clone());
                        }

                        // Retrieve the record.
                        let record = match &input {
                            Value::Record(record) => record,
                            // Ensure the input is a record.
                            Value::Plaintext(..) => A::halt("Expected an external record input, found a plaintext input"),
                        };

                        // Prepare the index as a constant field element.
                        let input_index = Field::constant(console::Field::from_u16(index as u16));
                        // Construct the preimage as `(function ID || input || tvk || index)`.
                        let mut preimage = vec![function_id.clone()];
                        preimage.extend(record.to_fields());
                        preimage.push(tvk.clone());
                        preimage.push(input_index);

                        // Ensure the expected hash matches the computed hash.
                        input_hash.is_equal(&A::hash_psd8(&preimage))
                    }
                }
            })
            .fold(Boolean::constant(true), |acc, x| acc & x);

        // Return the boolean, and (optional) the message.
        match CREATE_MESSAGE {
            true => (input_checks, Some(message)),
            false => match message.is_empty() {
                true => (input_checks, None),
                false => A::halt("Malformed synthesis of the logic to check input IDs"),
            },
        }
    }
}

#[cfg(all(test, console))]
mod tests {
    use super::*;
    use crate::Circuit;
    use snarkvm_utilities::TestRng;

    use anyhow::Result;

    pub(crate) const ITERATIONS: usize = 50;

    fn check_verify(
        mode: Mode,
        num_constants: u64,
        num_public: u64,
        num_private: u64,
        num_constraints: u64,
    ) -> Result<()> {
        let rng = &mut TestRng::default();

        for i in 0..ITERATIONS {
            // Sample a random private key and address.
            let private_key = snarkvm_console_account::PrivateKey::<<Circuit as Environment>::Network>::new(rng)?;
            let address = snarkvm_console_account::Address::try_from(&private_key).unwrap();

            // Construct a program ID and function name.
            let program_id = console::ProgramID::from_str("token.aleo")?;
            let function_name = console::Identifier::from_str("transfer")?;

            // Prepare a record belonging to the address.
            let record_string = format!(
                "{{ owner: {address}.private, gates: 5u64.private, token_amount: 100u64.private, _nonce: 0group.public }}"
            );

            // Construct the inputs.
            let input_constant =
                console::Value::<<Circuit as Environment>::Network>::from_str("{ token_amount: 9876543210u128 }")
                    .unwrap();
            let input_public =
                console::Value::<<Circuit as Environment>::Network>::from_str("{ token_amount: 9876543210u128 }")
                    .unwrap();
            let input_private =
                console::Value::<<Circuit as Environment>::Network>::from_str("{ token_amount: 9876543210u128 }")
                    .unwrap();
            let input_record = console::Value::<<Circuit as Environment>::Network>::from_str(&record_string).unwrap();
            let input_external_record =
                console::Value::<<Circuit as Environment>::Network>::from_str(&record_string).unwrap();
            let inputs = [input_constant, input_public, input_private, input_record, input_external_record];

            // Construct the input types.
            let input_types = vec![
                console::ValueType::from_str("amount.constant").unwrap(),
                console::ValueType::from_str("amount.public").unwrap(),
                console::ValueType::from_str("amount.private").unwrap(),
                console::ValueType::from_str("token.record").unwrap(),
                console::ValueType::from_str("token.aleo/token.record").unwrap(),
            ];

            // Compute the signed request.
            let request =
                console::Request::sign(&private_key, program_id, function_name, inputs.iter(), &input_types, rng)?;
            assert!(request.verify(&input_types));

            // Inject the request into a circuit.
            let tpk = Group::<Circuit>::new(mode, request.to_tpk());
            let request = Request::<Circuit>::new(mode, request);

            Circuit::scope(format!("Request {i}"), || {
                let candidate = request.verify(&input_types, &tpk);
                assert!(candidate.eject_value());
                match mode.is_constant() {
                    true => assert_scope!(<=num_constants, <=num_public, <=num_private, <=num_constraints),
                    false => assert_scope!(<=num_constants, num_public, num_private, num_constraints),
                }
            });

            Circuit::scope(format!("Request {i}"), || {
                let (candidate, _) = Request::check_input_ids::<false>(
                    request.network_id(),
                    request.program_id(),
                    request.function_name(),
                    request.input_ids(),
                    request.inputs(),
                    &input_types,
                    request.caller(),
                    request.sk_tag(),
                    request.tvk(),
                    request.tcm(),
                    None,
                );
                assert!(candidate.eject_value());
            });
            Circuit::reset();
        }
        Ok(())
    }

    #[test]
    fn test_sign_and_verify_constant() -> Result<()> {
        // Note: This is correct. At this (high) level of a program, we override the default mode in the `Record` case,
        // based on the user-defined visibility in the record type. Thus, we have nonzero private and constraint values.
        // These bounds are determined experimentally.
        check_verify(Mode::Constant, 46000, 0, 15100, 15100)
    }

    #[test]
    fn test_sign_and_verify_public() -> Result<()> {
        check_verify(Mode::Public, 41170, 0, 28119, 28153)
    }

    #[test]
    fn test_sign_and_verify_private() -> Result<()> {
        check_verify(Mode::Private, 41170, 0, 28119, 28153)
    }
}
