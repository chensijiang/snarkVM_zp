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

impl<N: Network> Serialize for CoinbaseSolution<N> {
    /// Serializes the coinbase solution to a JSON-string or buffer.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match serializer.is_human_readable() {
            true => {
                let mut combined_puzzle_solution = serializer.serialize_struct("CoinbaseSolution", 3)?;
                combined_puzzle_solution.serialize_field("partial_solutions", &self.partial_solutions)?;
                combined_puzzle_solution.serialize_field("proof.w", &self.proof.w)?;
                if let Some(random_v) = &self.proof.random_v {
                    combined_puzzle_solution.serialize_field("proof.random_v", &random_v)?;
                }
                combined_puzzle_solution.end()
            }
            false => ToBytesSerializer::serialize_with_size_encoding(self, serializer),
        }
    }
}

impl<'de, N: Network> Deserialize<'de> for CoinbaseSolution<N> {
    /// Deserializes the coinbase solution from a JSON-string or buffer.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserializer.is_human_readable() {
            true => {
                let mut combined_puzzle_solution = serde_json::Value::deserialize(deserializer)?;
                Ok(Self::new(
                    serde_json::from_value(combined_puzzle_solution["partial_solutions"].take())
                        .map_err(de::Error::custom)?,
                    KZGProof {
                        w: serde_json::from_value(combined_puzzle_solution["proof.w"].take())
                            .map_err(de::Error::custom)?,
                        random_v: match combined_puzzle_solution.get("proof.random_v") {
                            Some(random_v) => {
                                Some(serde_json::from_value(random_v.clone()).map_err(de::Error::custom)?)
                            }
                            None => None,
                        },
                    },
                ))
            }
            false => FromBytesDeserializer::<Self>::deserialize_with_size_encoding(deserializer, "coinbase solution"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console::{account::PrivateKey, network::Testnet3};

    type CurrentNetwork = Testnet3;

    #[test]
    fn test_serde_json() -> Result<()> {
        let mut rng = TestRng::default();

        // Sample a new coinbase solution.
        let mut partial_solutions = vec![];
        for _ in 0..rng.gen_range(1..10) {
            let private_key = PrivateKey::<CurrentNetwork>::new(&mut rng)?;
            let address = Address::try_from(private_key)?;

            partial_solutions.push(PartialSolution::new(address, u64::rand(&mut rng), KZGCommitment(rng.gen())));
        }
        let expected = CoinbaseSolution::new(partial_solutions, KZGProof { w: rng.gen(), random_v: None });

        // Serialize
        let expected_string = &expected.to_string();
        let candidate_string = serde_json::to_string(&expected)?;
        assert_eq!(expected, serde_json::from_str(&candidate_string)?);

        // Deserialize
        assert_eq!(expected, CoinbaseSolution::from_str(expected_string)?);
        assert_eq!(expected, serde_json::from_str(&candidate_string)?);

        Ok(())
    }

    #[test]
    fn test_bincode() -> Result<()> {
        let mut rng = TestRng::default();

        // Sample a new coinbase solution.
        let mut partial_solutions = vec![];
        for _ in 0..rng.gen_range(1..10) {
            let private_key = PrivateKey::<CurrentNetwork>::new(&mut rng)?;
            let address = Address::try_from(private_key)?;

            partial_solutions.push(PartialSolution::new(address, u64::rand(&mut rng), KZGCommitment(rng.gen())));
        }
        let expected = CoinbaseSolution::new(partial_solutions, KZGProof { w: rng.gen(), random_v: None });

        // Serialize
        let expected_bytes = expected.to_bytes_le()?;
        let expected_bytes_with_size_encoding = bincode::serialize(&expected)?;
        assert_eq!(&expected_bytes[..], &expected_bytes_with_size_encoding[8..]);

        // Deserialize
        assert_eq!(expected, CoinbaseSolution::read_le(&expected_bytes[..])?);
        assert_eq!(expected, bincode::deserialize(&expected_bytes_with_size_encoding[..])?);

        Ok(())
    }
}
