use alloy::primitives::U256;
use async_trait::async_trait;
use eyre::Result;
use log::info;
use std::sync::Arc;

use crate::{
    domain::transaction::Transaction,
    jobs::{JobProducer, TransactionSend, TransactionStatusCheck},
    models::{
        produce_transaction_update_notification_payload, RelayerRepoModel, TransactionError,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{InMemoryTransactionRepository, RelayerRepositoryStorage},
    services::{EvmProvider, GasPriceService, TransactionCounterService},
};
#[allow(dead_code)]
pub struct TransactionPriceParams {
    pub gas_price: Option<U256>,
    pub balance: Option<U256>,
}

#[allow(dead_code)]
pub struct EvmRelayerTransaction {
    relayer: RelayerRepoModel,
    provider: EvmProvider,
    relayer_repository: Arc<RelayerRepositoryStorage>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    transaction_counter_service: TransactionCounterService,
    job_producer: Arc<JobProducer>,
    gas_price_service: Arc<GasPriceService>,
}

#[allow(dead_code)]
impl EvmRelayerTransaction {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: EvmProvider,
        relayer_repository: Arc<RelayerRepositoryStorage>,
        transaction_repository: Arc<InMemoryTransactionRepository>,
        transaction_counter_service: TransactionCounterService,
        job_producer: Arc<JobProducer>,
        gas_price_service: Arc<GasPriceService>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            relayer,
            provider,
            relayer_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
            gas_price_service,
        })
    }
}

#[async_trait]
impl Transaction for EvmRelayerTransaction {
    async fn prepare_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("Preparing transaction");
        // validate the transaction
        let mut evm_data: crate::models::EvmTransactionData =
            tx.network_data.get_evm_transaction_data()?;
        let price_params = get_transaction_price_params(self, &tx, &self.relayer).await?;
        info!("Gas price: {:?}", price_params.gas_price);
        if let Some(gas_price) = price_params.gas_price {
            evm_data.gas_price = Some(gas_price.to::<u128>());
        }
        let gas_estimation = self.gas_price_service.estimate_gas(&evm_data).await?;
        info!("Gas estimation: {:?}", gas_estimation);
        // After preparing the transaction, submit it to the job queue
        self.job_producer
            .produce_submit_transaction_job(
                TransactionSend::submit(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await?;
        let updated = self
            .transaction_repository
            .update_status(tx.id.clone(), TransactionStatus::Sent)
            .await?;

        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated),
                    None,
                )
                .await?;
        }

        Ok(tx)
    }

    async fn submit_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        info!("submitting transaction");

        let updated = self
            .transaction_repository
            .update_status(tx.id.clone(), TransactionStatus::Submitted)
            .await?;

        // after submitting the transaction, we need to handle the transaction status
        self.job_producer
            .produce_check_transaction_status_job(
                TransactionStatusCheck::new(tx.id.clone(), tx.relayer_id.clone()),
                None,
            )
            .await?;

        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated),
                    None,
                )
                .await?;
        }
        Ok(tx)
    }

    async fn handle_transaction_status(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        let updated: TransactionRepoModel = self
            .transaction_repository
            .update_status(tx.id.clone(), TransactionStatus::Confirmed)
            .await?;

        if let Some(notification_id) = &self.relayer.notification_id {
            self.job_producer
                .produce_send_notification_job(
                    produce_transaction_update_notification_payload(notification_id, &updated),
                    None,
                )
                .await?;
        }
        Ok(tx)
    }

    async fn cancel_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn replace_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn sign_transaction(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, TransactionError> {
        Ok(tx)
    }

    async fn validate_transaction(
        &self,
        _tx: TransactionRepoModel,
    ) -> Result<bool, TransactionError> {
        Ok(true)
    }
}

/// Get the price params for the transaction based on getTransactionPriceParams defender
/// totalCost, balance, priceParams, extraCost
pub async fn get_transaction_price_params(
    evm_relayer_transaction: &EvmRelayerTransaction,
    tx: &TransactionRepoModel,
    _relayer: &RelayerRepoModel,
) -> Result<TransactionPriceParams, TransactionError> {
    let tx_data: crate::models::EvmTransactionData = tx.network_data.get_evm_transaction_data()?;
    let gas_price = match &tx_data.speed {
        Some(speed) => evm_relayer_transaction
            .gas_price_service
            .get_legacy_prices_from_json_rpc()
            .await?
            .into_iter()
            .find(|(s, _)| s == speed)
            .map(|(_, gas_price)| gas_price)
            .ok_or(TransactionError::NotSupported(
                "Not supported speed".to_string(),
            ))?,
        None => U256::from(tx_data.gas_price.ok_or(TransactionError::NotSupported(
            "Gas price is required".to_string(),
        ))?),
    };

    Ok(TransactionPriceParams {
        gas_price: Some(gas_price),
        balance: None,
    })
}
