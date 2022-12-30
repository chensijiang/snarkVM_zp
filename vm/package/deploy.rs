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

use crate::synthesizer::Deployment;
use snarkvm_console::types::Address;

use super::*;

pub struct DeployRequest<N: Network> {
    deployment: Deployment<N>,
    address: Address<N>,
    program_id: ProgramID<N>,
}

impl<N: Network> DeployRequest<N> {
    /// Sends the request to the given endpoint.
    pub fn new(deployment: Deployment<N>, address: Address<N>, program_id: ProgramID<N>) -> Self {
        Self { deployment, address, program_id }
    }

    /// Sends the request to the given endpoint.
    pub fn send(&self, endpoint: &str) -> Result<DeployResponse<N>> {
        Ok(ureq::post(endpoint).send_json(self)?.into_json()?)
    }

    /// Returns the program.
    pub const fn deployment(&self) -> &Deployment<N> {
        &self.deployment
    }

    /// Returns the program address.
    pub const fn address(&self) -> &Address<N> {
        &self.address
    }

    /// Returns the imports.
    pub const fn program_id(&self) -> &ProgramID<N> {
        &self.program_id
    }
}

impl<N: Network> Serialize for DeployRequest<N> {
    /// Serializes the deploy request into string or bytes.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut request = serializer.serialize_struct("DeployRequest", 3)?;
        // Serialize the deployment.
        request.serialize_field("deployment", &self.deployment)?;
        // Serialize the address.
        request.serialize_field("address", &self.address)?;
        // Serialize the program id.
        request.serialize_field("program_id", &self.program_id)?;
        request.end()
    }
}

impl<'de, N: Network> Deserialize<'de> for DeployRequest<N> {
    /// Deserializes the deploy request from a string or bytes.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parse the request from a string into a value.
        let mut request = serde_json::Value::deserialize(deserializer)?;
        // Recover the leaf.
        Ok(Self::new(
            // Retrieve the program.
            serde_json::from_value(request["deployment"].take()).map_err(de::Error::custom)?,
            // Retrieve the address of the program.
            serde_json::from_value(request["address"].take()).map_err(de::Error::custom)?,
            // Retrieve the program ID.
            serde_json::from_value(request["program_id"].take()).map_err(de::Error::custom)?,
        ))
    }
}

pub struct DeployResponse<N: Network> {
    deployment: Deployment<N>,
}

impl<N: Network> DeployResponse<N> {
    /// Initializes a new deploy response.
    pub const fn new(deployment: Deployment<N>) -> Self {
        Self { deployment }
    }

    /// Returns the program ID.
    pub const fn deployment(&self) -> &Deployment<N> {
        &self.deployment
    }
}

impl<N: Network> Serialize for DeployResponse<N> {
    /// Serializes the deploy response into string or bytes.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut response = serializer.serialize_struct("DeployResponse", 1)?;
        response.serialize_field("deployment", &self.deployment)?;
        response.end()
    }
}

impl<'de, N: Network> Deserialize<'de> for DeployResponse<N> {
    /// Deserializes the deploy response from a string or bytes.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parse the response from a string into a value.
        let mut response = serde_json::Value::deserialize(deserializer)?;
        // Recover the leaf.
        Ok(Self::new(
            // Retrieve the program ID.
            serde_json::from_value(response["deployment"].take()).map_err(de::Error::custom)?,
        ))
    }
}

impl<N: Network> Package<N> {
    pub fn deploy<A: crate::circuit::Aleo<Network = N, BaseField = N::Field>>(
        &self,
        endpoint: Option<String>,
    ) -> Result<Deployment<N>> {
        // Retrieve the main program.
        let program = self.program();
        // Retrieve the main program ID.
        let program_id = program.id();

        // Retrieve the Aleo address of the deployment caller.
        let caller = self.manifest_file().development_address();

        #[cfg(feature = "aleo-cli")]
        println!("⏳ Deploying '{}'...\n", program_id.to_string().bold());

        // Construct the process.
        let mut process = Process::<N>::load()?;

        // Add program imports to the process.
        let imports_directory = self.imports_directory();
        program.imports().keys().try_for_each(|program_id| {
            // TODO (howardwu): Add the following checks:
            //  1) the imported program ID exists *on-chain* (for the given network)
            //  2) the AVM bytecode of the imported program matches the AVM bytecode of the program *on-chain*
            //  3) consensus performs the exact same checks (in `verify_deployment`)

            // Open the Aleo program file.
            let import_program_file = AleoFile::open(&imports_directory, program_id, false)?;
            // Add the import program.
            process.add_program(import_program_file.program())?;
            Ok::<_, Error>(())
        })?;

        // Initialize the RNG.
        let rng = &mut rand::thread_rng();
        // Compute the deployment.
        let deployment = process.deploy::<A, _>(program, rng).unwrap();

        match endpoint {
            Some(ref endpoint) => {
                // Construct the deploy request.
                let request = DeployRequest::new(deployment, *caller, *program_id);
                // Send the deploy request.
                let response = request.send(endpoint)?;
                // Ensure the program ID matches.
                ensure!(
                    response.deployment.program_id() == program_id,
                    "Program ID mismatch: {} != {program_id}",
                    response.deployment.program_id()
                );
                Ok(response.deployment)
            }
            None => Ok(deployment),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type CurrentNetwork = snarkvm_console::network::Testnet3;
    type CurrentAleo = snarkvm_circuit::network::AleoV0;

    #[test]
    fn test_deploy() {
        // Samples a new package at a temporary directory.
        let (directory, package) = crate::package::test_helpers::sample_package();

        // Deploy the package.
        let deployment = package.deploy::<CurrentAleo>(None).unwrap();

        // Ensure the deployment edition matches.
        assert_eq!(<CurrentNetwork as Network>::EDITION, deployment.edition());
        // Ensure the deployment program ID matches.
        assert_eq!(package.program().id(), deployment.program_id());
        // Ensure the deployment program matches.
        assert_eq!(package.program(), deployment.program());

        // Proactively remove the temporary directory (to conserve space).
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn test_deploy_with_import() {
        // Samples a new package at a temporary directory.
        let (directory, package) = crate::package::test_helpers::sample_package_with_import();

        // Deploy the package.
        let deployment = package.deploy::<CurrentAleo>(None).unwrap();

        // Ensure the deployment edition matches.
        assert_eq!(<CurrentNetwork as Network>::EDITION, deployment.edition());
        // Ensure the deployment program ID matches.
        assert_eq!(package.program().id(), deployment.program_id());
        // Ensure the deployment program matches.
        assert_eq!(package.program(), deployment.program());

        // Proactively remove the temporary directory (to conserve space).
        std::fs::remove_dir_all(directory).unwrap();
    }
}
