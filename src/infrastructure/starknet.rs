use async_trait::async_trait;
use log::{error, info};
use starknet::{
    accounts::{Account, Call, SingleOwnerAccount},
    core::{
        chain_id,
        types::{BlockId, CallFunction, FieldElement},
    },
    macros::selector,
    providers::{Provider, SequencerGatewayProvider},
    signers::{LocalWallet, SigningKey},
};
use std::sync::Arc;

use crate::domain::bridge::{MintError, StarknetManager};

pub struct OnChainStartknetManager {
    provider: Arc<SequencerGatewayProvider>,
    account_address: String,
    account_private_key: String,
}

impl OnChainStartknetManager {
    pub fn new(
        provider: Arc<SequencerGatewayProvider>,
        account_addr: &str,
        account_pk: &str,
    ) -> Self {
        Self {
            provider,
            account_address: account_addr.to_string(),
            account_private_key: account_pk.to_string(),
        }
    }
}

#[async_trait]
impl StarknetManager for OnChainStartknetManager {
    async fn project_has_token(&self, project_id: &str, token_id: &str) -> bool {
        let provider = self.provider.clone();

        let res = provider
            .call_contract(
                CallFunction {
                    contract_address: FieldElement::from_hex_be(project_id).unwrap(),
                    entry_point_selector: selector!("ownerOf"),
                    calldata: vec![
                        FieldElement::from_dec_str(token_id).unwrap(),
                        FieldElement::ZERO,
                    ],
                },
                BlockId::Latest,
            )
            .await;

        res.is_ok()
    }

    async fn mint_project_token(
        &self,
        project_id: &str,
        token_id: &str,
        starknet_account_addr: &str,
    ) -> Result<String, MintError> {
        info!(
            "Trying to mint token {} on project {}",
            token_id, project_id
        );
        let provider = self.provider.clone();
        let signer = LocalWallet::from(SigningKey::from_secret_scalar(
            FieldElement::from_hex_be(self.account_private_key.as_str()).unwrap(),
        ));

        let address = FieldElement::from_hex_be(self.account_address.as_str()).unwrap();
        let to = FieldElement::from_hex_be(starknet_account_addr).unwrap();

        let account = SingleOwnerAccount::new(provider, signer, address, chain_id::TESTNET);

        let res = account
            .execute(&[Call {
                to: FieldElement::from_hex_be(project_id).unwrap(),
                selector: selector!("mint"),
                calldata: vec![
                    to,
                    FieldElement::from_dec_str(token_id).unwrap(),
                    FieldElement::ZERO,
                ],
            }])
            .send()
            .await;

        match res {
            Ok(tx) => {
                info!(
                    "Token id {} minting in progress -> #{}",
                    token_id,
                    tx.transaction_hash.to_string()
                );
                Ok(tx.transaction_hash.to_string())
            }
            Err(e) => {
                error!(
                    "Error while minting token id {} -> {}",
                    token_id,
                    e.to_string()
                );
                Err(MintError::Failure)
            }
        }
    }
}
