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

use crate::{
    block::Output,
    store::helpers::{memory_map::MemoryMap, Map, MapRead},
};
use console::{
    network::prelude::*,
    program::{Ciphertext, Plaintext, Record},
    types::{Field, Group},
};

use anyhow::Result;
use std::borrow::Cow;

/// A trait for transition output storage.
pub trait OutputStorage<N: Network>: Clone + Send + Sync {
    /// The mapping of `transition ID` to `output IDs`.
    type IDMap: for<'a> Map<'a, N::TransitionID, Vec<Field<N>>>;
    /// The mapping of `output ID` to `transition ID`.
    type ReverseIDMap: for<'a> Map<'a, Field<N>, N::TransitionID>;
    /// The mapping of `plaintext hash` to `(optional) plaintext`.
    type ConstantMap: for<'a> Map<'a, Field<N>, Option<Plaintext<N>>>;
    /// The mapping of `plaintext hash` to `(optional) plaintext`.
    type PublicMap: for<'a> Map<'a, Field<N>, Option<Plaintext<N>>>;
    /// The mapping of `ciphertext hash` to `(optional) ciphertext`.
    type PrivateMap: for<'a> Map<'a, Field<N>, Option<Ciphertext<N>>>;
    /// The mapping of `commitment` to `(checksum, (optional) record ciphertext)`.
    type RecordMap: for<'a> Map<'a, Field<N>, (Field<N>, Option<Record<N, Ciphertext<N>>>)>;
    /// The mapping of `record nonce` to `commitment`.
    type RecordNonceMap: for<'a> Map<'a, Group<N>, Field<N>>;
    /// The mapping of `external hash` to `()`. Note: This is **not** the record commitment.
    type ExternalRecordMap: for<'a> Map<'a, Field<N>, ()>;

    /// Initializes the transition output storage.
    fn open(dev: Option<u16>) -> Result<Self>;

    /// Returns the ID map.
    fn id_map(&self) -> &Self::IDMap;
    /// Returns the reverse ID map.
    fn reverse_id_map(&self) -> &Self::ReverseIDMap;
    /// Returns the constant map.
    fn constant_map(&self) -> &Self::ConstantMap;
    /// Returns the public map.
    fn public_map(&self) -> &Self::PublicMap;
    /// Returns the private map.
    fn private_map(&self) -> &Self::PrivateMap;
    /// Returns the record map.
    fn record_map(&self) -> &Self::RecordMap;
    /// Returns the record nonce map.
    fn record_nonce_map(&self) -> &Self::RecordNonceMap;
    /// Returns the external record map.
    fn external_record_map(&self) -> &Self::ExternalRecordMap;

    /// Returns the optional development ID.
    fn dev(&self) -> Option<u16>;

    /// Starts an atomic batch write operation.
    fn start_atomic(&self) {
        self.id_map().start_atomic();
        self.reverse_id_map().start_atomic();
        self.constant_map().start_atomic();
        self.public_map().start_atomic();
        self.private_map().start_atomic();
        self.record_map().start_atomic();
        self.record_nonce_map().start_atomic();
        self.external_record_map().start_atomic();
    }

    /// Checks if an atomic batch is in progress.
    fn is_atomic_in_progress(&self) -> bool {
        self.id_map().is_atomic_in_progress()
            || self.reverse_id_map().is_atomic_in_progress()
            || self.constant_map().is_atomic_in_progress()
            || self.public_map().is_atomic_in_progress()
            || self.private_map().is_atomic_in_progress()
            || self.record_map().is_atomic_in_progress()
            || self.record_nonce_map().is_atomic_in_progress()
            || self.external_record_map().is_atomic_in_progress()
    }

    /// Aborts an atomic batch write operation.
    fn abort_atomic(&self) {
        self.id_map().abort_atomic();
        self.reverse_id_map().abort_atomic();
        self.constant_map().abort_atomic();
        self.public_map().abort_atomic();
        self.private_map().abort_atomic();
        self.record_map().abort_atomic();
        self.record_nonce_map().abort_atomic();
        self.external_record_map().abort_atomic();
    }

    /// Finishes an atomic batch write operation.
    fn finish_atomic(&self) -> Result<()> {
        self.id_map().finish_atomic()?;
        self.reverse_id_map().finish_atomic()?;
        self.constant_map().finish_atomic()?;
        self.public_map().finish_atomic()?;
        self.private_map().finish_atomic()?;
        self.record_map().finish_atomic()?;
        self.record_nonce_map().finish_atomic()?;
        self.external_record_map().finish_atomic()
    }

    /// Stores the given `(transition ID, output)` pair into storage.
    fn insert(&self, transition_id: N::TransitionID, outputs: &[Output<N>]) -> Result<()> {
        // Check if an atomic batch write is already in progress.
        let is_part_of_atomic_batch = self.is_atomic_in_progress();

        // Start an atomic batch write operation IFF it's not already part of one.
        if !is_part_of_atomic_batch {
            self.start_atomic();
        }

        let run_atomic_ops = || -> Result<()> {
            // Store the output IDs.
            self.id_map().insert(transition_id, outputs.iter().map(Output::id).copied().collect())?;

            // Store the outputs.
            for output in outputs {
                // Store the reverse output ID.
                self.reverse_id_map().insert(*output.id(), transition_id)?;
                // Store the output.
                match output.clone() {
                    Output::Constant(output_id, constant) => self.constant_map().insert(output_id, constant)?,
                    Output::Public(output_id, public) => self.public_map().insert(output_id, public)?,
                    Output::Private(output_id, private) => self.private_map().insert(output_id, private)?,
                    Output::Record(commitment, checksum, optional_record) => {
                        // If the optional record exists, insert the record nonce.
                        if let Some(record) = &optional_record {
                            self.record_nonce_map().insert(*record.nonce(), commitment)?;
                        }
                        // Insert the record entry.
                        self.record_map().insert(commitment, (checksum, optional_record))?
                    }
                    Output::ExternalRecord(output_id) => self.external_record_map().insert(output_id, ())?,
                }
            }

            Ok(())
        };

        // Abort if any of the underlying operations has failed.
        run_atomic_ops().map_err(|err| {
            self.abort_atomic();
            err
        })?;

        // Finish an atomic batch write operation IFF it's not already part of one.
        if !is_part_of_atomic_batch {
            self.finish_atomic()?;
        }

        Ok(())
    }

    /// Removes the output for the given `transition ID`.
    fn remove(&self, transition_id: &N::TransitionID) -> Result<()> {
        // Retrieve the output IDs.
        let output_ids: Vec<_> = match self.id_map().get(transition_id)? {
            Some(Cow::Borrowed(ids)) => ids.to_vec(),
            Some(Cow::Owned(ids)) => ids.into_iter().collect(),
            None => return Ok(()),
        };

        // Check if an atomic batch write is already in progress.
        let is_part_of_atomic_batch = self.is_atomic_in_progress();

        // Start an atomic batch write operation IFF it's not already part of one.
        if !is_part_of_atomic_batch {
            self.start_atomic();
        }

        let run_atomic_ops = || -> Result<()> {
            // Remove the output IDs.
            self.id_map().remove(transition_id)?;

            // Remove the outputs.
            for output_id in output_ids {
                // Remove the reverse output ID.
                self.reverse_id_map().remove(&output_id)?;

                // If the output is a record, remove the record nonce.
                if let Some(record) = self.record_map().get(&output_id)? {
                    if let Some(record) = &record.1 {
                        self.record_nonce_map().remove(record.nonce())?;
                    }
                }

                // Remove the output.
                self.constant_map().remove(&output_id)?;
                self.public_map().remove(&output_id)?;
                self.private_map().remove(&output_id)?;
                self.record_map().remove(&output_id)?;
                self.external_record_map().remove(&output_id)?;
            }

            Ok(())
        };

        // Abort if any of the underlying operations has failed.
        run_atomic_ops().map_err(|err| {
            self.abort_atomic();
            err
        })?;

        // Finish an atomic batch write operation IFF it's not already part of one.
        if !is_part_of_atomic_batch {
            self.finish_atomic()?;
        }

        Ok(())
    }

    /// Returns the transition ID that contains the given `output ID`.
    fn find_transition_id(&self, output_id: &Field<N>) -> Result<Option<N::TransitionID>> {
        match self.reverse_id_map().get(output_id)? {
            Some(Cow::Borrowed(transition_id)) => Ok(Some(*transition_id)),
            Some(Cow::Owned(transition_id)) => Ok(Some(transition_id)),
            None => Ok(None),
        }
    }

    /// Returns the output IDs for the given `transition ID`.
    fn get_ids(&self, transition_id: &N::TransitionID) -> Result<Vec<Field<N>>> {
        // Retrieve the output IDs.
        match self.id_map().get(transition_id)? {
            Some(Cow::Borrowed(outputs)) => Ok(outputs.to_vec()),
            Some(Cow::Owned(outputs)) => Ok(outputs),
            None => Ok(vec![]),
        }
    }

    /// Returns the output for the given `transition ID`.
    fn get(&self, transition_id: &N::TransitionID) -> Result<Vec<Output<N>>> {
        // Constructs the output given the output ID and output value.
        macro_rules! into_output {
            (Output::Record($output_id:ident, $output:expr)) => {
                match $output {
                    Cow::Borrowed((checksum, opt_record)) => Output::Record($output_id, *checksum, opt_record.clone()),
                    Cow::Owned((checksum, opt_record)) => Output::Record($output_id, checksum, opt_record),
                }
            };
            (Output::$Variant:ident($output_id:ident, $output:expr)) => {
                match $output {
                    Cow::Borrowed(output) => Output::$Variant($output_id, output.clone()),
                    Cow::Owned(output) => Output::$Variant($output_id, output),
                }
            };
        }

        // A helper function to construct the output given the output ID.
        let construct_output = |output_id| {
            let constant = self.constant_map().get(&output_id)?;
            let public = self.public_map().get(&output_id)?;
            let private = self.private_map().get(&output_id)?;
            let record = self.record_map().get(&output_id)?;
            let external_record = self.external_record_map().get(&output_id)?;

            // Retrieve the output.
            let output = match (constant, public, private, record, external_record) {
                (Some(constant), None, None, None, None) => into_output!(Output::Constant(output_id, constant)),
                (None, Some(public), None, None, None) => into_output!(Output::Public(output_id, public)),
                (None, None, Some(private), None, None) => into_output!(Output::Private(output_id, private)),
                (None, None, None, Some(record), None) => into_output!(Output::Record(output_id, record)),
                (None, None, None, None, Some(_)) => Output::ExternalRecord(output_id),
                (None, None, None, None, None) => bail!("Missing output '{output_id}' in transition '{transition_id}'"),
                _ => bail!("Found multiple outputs for the output ID '{output_id}' in transition '{transition_id}'"),
            };

            Ok(output)
        };

        // Retrieve the output IDs.
        match self.id_map().get(transition_id)? {
            Some(Cow::Borrowed(ids)) => ids.iter().map(|output_id| construct_output(*output_id)).collect(),
            Some(Cow::Owned(ids)) => ids.iter().map(|output_id| construct_output(*output_id)).collect(),
            None => Ok(vec![]),
        }
    }
}

/// An in-memory transition output storage.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct OutputMemory<N: Network> {
    /// The mapping of `transition ID` to `output IDs`.
    id_map: MemoryMap<N::TransitionID, Vec<Field<N>>>,
    /// The mapping of `output ID` to `transition ID`.
    reverse_id_map: MemoryMap<Field<N>, N::TransitionID>,
    /// The mapping of `plaintext hash` to `(optional) plaintext`.
    constant: MemoryMap<Field<N>, Option<Plaintext<N>>>,
    /// The mapping of `plaintext hash` to `(optional) plaintext`.
    public: MemoryMap<Field<N>, Option<Plaintext<N>>>,
    /// The mapping of `ciphertext hash` to `(optional) ciphertext`.
    private: MemoryMap<Field<N>, Option<Ciphertext<N>>>,
    /// The mapping of `commitment` to `(checksum, (optional) record ciphertext)`.
    record: MemoryMap<Field<N>, (Field<N>, Option<Record<N, Ciphertext<N>>>)>,
    /// The mapping of `record nonce` to `commitment`.
    record_nonce: MemoryMap<Group<N>, Field<N>>,
    /// The mapping of `external hash` to `()`. Note: This is **not** the record commitment.
    external_record: MemoryMap<Field<N>, ()>,
    /// The optional development ID.
    dev: Option<u16>,
}

#[rustfmt::skip]
impl<N: Network> OutputStorage<N> for OutputMemory<N> {
    type IDMap = MemoryMap<N::TransitionID, Vec<Field<N>>>;
    type ReverseIDMap = MemoryMap<Field<N>, N::TransitionID>;
    type ConstantMap = MemoryMap<Field<N>, Option<Plaintext<N>>>;
    type PublicMap = MemoryMap<Field<N>, Option<Plaintext<N>>>;
    type PrivateMap = MemoryMap<Field<N>, Option<Ciphertext<N>>>;
    type RecordMap = MemoryMap<Field<N>, (Field<N>, Option<Record<N, Ciphertext<N>>>)>;
    type RecordNonceMap = MemoryMap<Group<N>, Field<N>>;
    type ExternalRecordMap = MemoryMap<Field<N>, ()>;

    /// Initializes the transition output storage.
    fn open(dev: Option<u16>) -> Result<Self> {
        Ok(Self {
            id_map: Default::default(),
            reverse_id_map: Default::default(),
            constant: Default::default(),
            public: Default::default(),
            private: Default::default(),
            record: Default::default(),
            record_nonce: Default::default(),
            external_record: Default::default(),
            dev,
        })
    }

    /// Returns the ID map.
    fn id_map(&self) -> &Self::IDMap {
        &self.id_map
    }

    /// Returns the reverse ID map.
    fn reverse_id_map(&self) -> &Self::ReverseIDMap {
        &self.reverse_id_map
    }

    /// Returns the constant map.
    fn constant_map(&self) -> &Self::ConstantMap {
        &self.constant
    }

    /// Returns the public map.
    fn public_map(&self) -> &Self::PublicMap {
        &self.public
    }

    /// Returns the private map.
    fn private_map(&self) -> &Self::PrivateMap {
        &self.private
    }

    /// Returns the record map.
    fn record_map(&self) -> &Self::RecordMap {
        &self.record
    }

    /// Returns the record nonce map.
    fn record_nonce_map(&self) -> &Self::RecordNonceMap {
        &self.record_nonce
    }

    /// Returns the external record map.
    fn external_record_map(&self) -> &Self::ExternalRecordMap {
        &self.external_record
    }

    /// Returns the optional development ID.
    fn dev(&self) -> Option<u16> {
        self.dev
    }
}

/// The transition output store.
#[derive(Clone)]
pub struct OutputStore<N: Network, O: OutputStorage<N>> {
    /// The map of constant outputs.
    constant: O::ConstantMap,
    /// The map of public outputs.
    public: O::PublicMap,
    /// The map of private outputs.
    private: O::PrivateMap,
    /// The map of record outputs.
    record: O::RecordMap,
    /// The map of record nonces.
    record_nonce: O::RecordNonceMap,
    /// The map of external record outputs.
    external_record: O::ExternalRecordMap,
    /// The output storage.
    storage: O,
}

impl<N: Network, O: OutputStorage<N>> OutputStore<N, O> {
    /// Initializes the transition output store.
    pub fn open(dev: Option<u16>) -> Result<Self> {
        // Initialize a new transition output storage.
        let storage = O::open(dev)?;
        // Return the transition output store.
        Ok(Self {
            constant: storage.constant_map().clone(),
            public: storage.public_map().clone(),
            private: storage.private_map().clone(),
            record: storage.record_map().clone(),
            record_nonce: storage.record_nonce_map().clone(),
            external_record: storage.external_record_map().clone(),
            storage,
        })
    }

    /// Initializes a transition output store from storage.
    pub fn from(storage: O) -> Self {
        Self {
            constant: storage.constant_map().clone(),
            public: storage.public_map().clone(),
            private: storage.private_map().clone(),
            record: storage.record_map().clone(),
            record_nonce: storage.record_nonce_map().clone(),
            external_record: storage.external_record_map().clone(),
            storage,
        }
    }

    /// Stores the given `(transition ID, output)` pair into storage.
    pub fn insert(&self, transition_id: N::TransitionID, outputs: &[Output<N>]) -> Result<()> {
        self.storage.insert(transition_id, outputs)
    }

    /// Removes the output for the given `transition ID`.
    pub fn remove(&self, transition_id: &N::TransitionID) -> Result<()> {
        self.storage.remove(transition_id)
    }

    /// Starts an atomic batch write operation.
    pub fn start_atomic(&self) {
        self.storage.start_atomic();
    }

    /// Checks if an atomic batch is in progress.
    pub fn is_atomic_in_progress(&self) -> bool {
        self.storage.is_atomic_in_progress()
    }

    /// Aborts an atomic batch write operation.
    pub fn abort_atomic(&self) {
        self.storage.abort_atomic();
    }

    /// Finishes an atomic batch write operation.
    pub fn finish_atomic(&self) -> Result<()> {
        self.storage.finish_atomic()
    }

    /// Returns the optional development ID.
    pub fn dev(&self) -> Option<u16> {
        self.storage.dev()
    }
}

impl<N: Network, O: OutputStorage<N>> OutputStore<N, O> {
    /// Returns the output IDs for the given `transition ID`.
    pub fn get_output_ids(&self, transition_id: &N::TransitionID) -> Result<Vec<Field<N>>> {
        self.storage.get_ids(transition_id)
    }

    /// Returns the outputs for the given `transition ID`.
    pub fn get_outputs(&self, transition_id: &N::TransitionID) -> Result<Vec<Output<N>>> {
        self.storage.get(transition_id)
    }

    /// Returns the record for the given `commitment`.
    ///
    /// If the record exists, `Ok(Some(record))` is returned.
    /// If the record was purged, `Ok(None)` is returned.
    /// If the record does not exist, `Err(error)` is returned.
    pub fn get_record(&self, commitment: &Field<N>) -> Result<Option<Record<N, Ciphertext<N>>>> {
        match self.record.get(commitment) {
            Ok(Some(Cow::Borrowed((_, Some(record))))) => Ok(Some((*record).clone())),
            Ok(Some(Cow::Owned((_, Some(record))))) => Ok(Some(record)),
            Ok(Some(Cow::Borrowed((_, None)))) => Ok(None),
            Ok(Some(Cow::Owned((_, None)))) => Ok(None),
            Ok(None) => bail!("Record '{commitment}' not found"),
            Err(e) => Err(e),
        }
    }
}

impl<N: Network, O: OutputStorage<N>> OutputStore<N, O> {
    /// Returns the transition ID that contains the given `output ID`.
    pub fn find_transition_id(&self, output_id: &Field<N>) -> Result<Option<N::TransitionID>> {
        self.storage.find_transition_id(output_id)
    }
}

impl<N: Network, O: OutputStorage<N>> OutputStore<N, O> {
    /// Returns `true` if the given output ID exists.
    pub fn contains_output_id(&self, output_id: &Field<N>) -> Result<bool> {
        self.storage.reverse_id_map().contains_key(output_id)
    }

    /// Returns `true` if the given commitment exists.
    pub fn contains_commitment(&self, commitment: &Field<N>) -> Result<bool> {
        self.record.contains_key(commitment)
    }

    /// Returns `true` if the given checksum exists.
    pub fn contains_checksum(&self, checksum: &Field<N>) -> bool {
        self.checksums().contains(checksum)
    }

    /// Returns `true` if the given nonce exists.
    pub fn contains_nonce(&self, nonce: &Group<N>) -> Result<bool> {
        self.record_nonce.contains_key(nonce)
    }
}

impl<N: Network, O: OutputStorage<N>> OutputStore<N, O> {
    /// Returns an iterator over the output IDs, for all transition outputs.
    pub fn output_ids(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.storage.reverse_id_map().keys()
    }

    /// Returns an iterator over the constant output IDs, for all transition outputs that are constant.
    pub fn constant_output_ids(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.constant.keys()
    }

    /// Returns an iterator over the public output IDs, for all transition outputs that are public.
    pub fn public_output_ids(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.public.keys()
    }

    /// Returns an iterator over the private output IDs, for all transition outputs that are private.
    pub fn private_output_ids(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.private.keys()
    }

    /// Returns an iterator over the commitments, for all transition outputs that are records.
    pub fn commitments(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.record.keys()
    }

    /// Returns an iterator over the external record output IDs, for all transition outputs that are external records.
    pub fn external_output_ids(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.external_record.keys()
    }
}

impl<N: Network, I: OutputStorage<N>> OutputStore<N, I> {
    /// Returns an iterator over the constant outputs, for all transitions.
    pub fn constant_outputs(&self) -> impl '_ + Iterator<Item = Cow<'_, Plaintext<N>>> {
        self.constant.values().flat_map(|output| match output {
            Cow::Borrowed(Some(output)) => Some(Cow::Borrowed(output)),
            Cow::Owned(Some(output)) => Some(Cow::Owned(output)),
            _ => None,
        })
    }

    /// Returns an iterator over the constant outputs, for all transitions.
    pub fn public_outputs(&self) -> impl '_ + Iterator<Item = Cow<'_, Plaintext<N>>> {
        self.public.values().flat_map(|output| match output {
            Cow::Borrowed(Some(output)) => Some(Cow::Borrowed(output)),
            Cow::Owned(Some(output)) => Some(Cow::Owned(output)),
            _ => None,
        })
    }

    /// Returns an iterator over the private outputs, for all transitions.
    pub fn private_outputs(&self) -> impl '_ + Iterator<Item = Cow<'_, Ciphertext<N>>> {
        self.private.values().flat_map(|output| match output {
            Cow::Borrowed(Some(output)) => Some(Cow::Borrowed(output)),
            Cow::Owned(Some(output)) => Some(Cow::Owned(output)),
            _ => None,
        })
    }

    /// Returns an iterator over the checksums, for all transition outputs that are records.
    pub fn checksums(&self) -> impl '_ + Iterator<Item = Cow<'_, Field<N>>> {
        self.record.values().map(|output| match output {
            Cow::Borrowed((checksum, _)) => Cow::Borrowed(checksum),
            Cow::Owned((checksum, _)) => Cow::Owned(checksum),
        })
    }

    /// Returns an iterator over the nonces, for all transition outputs that are records.
    pub fn nonces(&self) -> impl '_ + Iterator<Item = Cow<'_, Group<N>>> {
        self.record_nonce.keys()
    }

    /// Returns an iterator over the `(commitment, record)` pairs, for all transition outputs that are records.
    pub fn records(&self) -> impl '_ + Iterator<Item = (Cow<'_, Field<N>>, Cow<'_, Record<N, Ciphertext<N>>>)> {
        self.record.iter().flat_map(|(commitment, output)| match output {
            Cow::Borrowed((_, Some(record))) => Some((commitment, Cow::Borrowed(record))),
            Cow::Owned((_, Some(record))) => Some((commitment, Cow::Owned(record))),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_get_remove() {
        // Sample the transition outputs.
        for (transition_id, output) in crate::block::transition::output::test_helpers::sample_outputs() {
            // Initialize a new output store.
            let output_store = OutputMemory::open(None).unwrap();

            // Ensure the transition output does not exist.
            let candidate = output_store.get(&transition_id).unwrap();
            assert!(candidate.is_empty());

            // Insert the transition output.
            output_store.insert(transition_id, &[output.clone()]).unwrap();

            // Retrieve the transition output.
            let candidate = output_store.get(&transition_id).unwrap();
            assert_eq!(vec![output.clone()], candidate);

            // Remove the transition output.
            output_store.remove(&transition_id).unwrap();

            // Retrieve the transition output.
            let candidate = output_store.get(&transition_id).unwrap();
            assert!(candidate.is_empty());
        }
    }

    #[test]
    fn test_find_transition_id() {
        // Sample the transition outputs.
        for (transition_id, output) in crate::block::transition::output::test_helpers::sample_outputs() {
            // Initialize a new output store.
            let output_store = OutputMemory::open(None).unwrap();

            // Ensure the transition output does not exist.
            let candidate = output_store.get(&transition_id).unwrap();
            assert!(candidate.is_empty());

            // Ensure the transition ID is not found.
            let candidate = output_store.find_transition_id(output.id()).unwrap();
            assert!(candidate.is_none());

            // Insert the transition output.
            output_store.insert(transition_id, &[output.clone()]).unwrap();

            // Find the transition ID.
            let candidate = output_store.find_transition_id(output.id()).unwrap();
            assert_eq!(Some(transition_id), candidate);

            // Remove the transition output.
            output_store.remove(&transition_id).unwrap();

            // Ensure the transition ID is not found.
            let candidate = output_store.find_transition_id(output.id()).unwrap();
            assert!(candidate.is_none());
        }
    }
}
