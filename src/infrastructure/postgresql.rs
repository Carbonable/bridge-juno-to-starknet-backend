use crate::domain::{
    bridge::{QueueError, QueueItem, QueueManager, QueueStatus},
    save_customer_data::{CustomerKeys, DataRepository, SaveCustomerDataError},
};
use async_trait::async_trait;
use cosmrs::proto::ics23::batch_entry;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use log::error;
use postgres_types::FromSql;
use std::sync::Arc;
use tokio_postgres::{Config, Error, NoTls};

pub async fn get_connection(database_uri: &str) -> core::result::Result<Pool, Error> {
    let config = database_uri.parse::<Config>()?;
    let manager_config = ManagerConfig {
        recycling_method: RecyclingMethod::Verified,
    };
    let manager = Manager::from_config(config, NoTls, manager_config);
    let pool = Pool::builder(manager).max_size(16).build().unwrap();

    Ok(pool)
}

pub struct PostgresDataRepository {
    connection_pool: Arc<Pool>,
}
impl PostgresDataRepository {
    pub fn new(connection_pool: Arc<Pool>) -> Self {
        Self { connection_pool }
    }
}

#[async_trait]
impl DataRepository for PostgresDataRepository {
    async fn save_customer_keys(&self, keys: CustomerKeys) -> Result<(), SaveCustomerDataError> {
        let client = self.connection_pool.clone().get().await.unwrap();

        let insert = client.execute(
            "INSERT INTO customer_keys (keplr_wallet_pubkey, project_id, token_ids) VALUES ($1, $2, $3)",
            &[&keys.keplr_wallet_pubkey, &keys.project_id, &keys.token_ids]
            ).await;
        if insert.is_err() {
            error!("Error while inserting customer to database {:#?}", insert);
            let update = client.execute(
                "UPDATE customer_keys SET token_ids = $1 WHERE keplr_wallet_pubkey = $2 AND project_id = $3",
                &[&keys.token_ids, &keys.keplr_wallet_pubkey, &keys.project_id]).await;

            if update.is_err() {
                error!("Error while saving customer to database {:#?}", update);
                return Err(SaveCustomerDataError::FailedToPersistToDatabase);
            }

            return Ok(());
        }

        if 1 == insert.unwrap() {
            return Ok(());
        }

        Err(SaveCustomerDataError::NotImpled)
    }

    async fn get_customer_keys(
        &self,
        keplr_wallet_pubkey: &str,
        project_id: &str,
    ) -> Result<CustomerKeys, SaveCustomerDataError> {
        let client = self.connection_pool.clone().get().await.unwrap();

        let query = client.prepare("SELECT * FROM customer_keys ck WHERE ck.keplr_wallet_pubkey = $1 AND ck.project_id = $2").await.unwrap();

        let rows = match client
            .query(&query, &[&keplr_wallet_pubkey, &project_id])
            .await
        {
            Ok(r) => r,
            Err(_e) => return Err(SaveCustomerDataError::NotFound),
        };
        if 0 == rows.len() {
            return Err(SaveCustomerDataError::NotFound);
        }
        let row = &rows[0];
        let customer_keys = CustomerKeys {
            keplr_wallet_pubkey: row.get::<usize, String>(1).into(),
            project_id: row.get::<usize, String>(2).into(),
            token_ids: row.get::<usize, Vec<String>>(3).into(),
        };

        Ok(customer_keys)
    }
}

#[derive(FromSql)]
#[postgres(name = "migration_status_values")]
pub enum PostgresQueueStatus {
    #[postgres(name = "pending")]
    Pending,
    #[postgres(name = "processing")]
    Processing,
    #[postgres(name = "success")]
    Success,
    #[postgres(name = "error")]
    Error,
}

impl From<PostgresQueueStatus> for QueueStatus {
    fn from(value: PostgresQueueStatus) -> Self {
        match value {
            PostgresQueueStatus::Pending => QueueStatus::Pending,
            PostgresQueueStatus::Processing => QueueStatus::Processing,
            PostgresQueueStatus::Success => QueueStatus::Success,
            PostgresQueueStatus::Error => QueueStatus::Error,
        }
    }
}

pub struct PostgresQueueManager {
    connection_pool: Arc<Pool>,
    batch_size: u8,
}

#[async_trait]
impl QueueManager for PostgresQueueManager {
    async fn enqueue(
        &self,
        keplr_wallet_pubkey: &str,
        project_id: &str,
        token_ids: Vec<String>,
    ) -> Result<Vec<QueueItem>, QueueError> {
        let mut client = self.connection_pool.clone().get().await.unwrap();

        let mut queue_items = Vec::new();
        let tx_builder = client.build_transaction();
        let tx = tx_builder.start().await.unwrap();
        for token in &token_ids {
            let _insert = tx.execute(
                "INSERT INTO migration_queue (keplr_wallet_pubkey, project_id, token_id) VALUES ($1, $2, $3)",
                &[&keplr_wallet_pubkey, &project_id, &token]
                ).await;

            queue_items.push(QueueItem::new(
                keplr_wallet_pubkey,
                project_id,
                token.to_string(),
            ));
        }

        match tx.commit().await {
            Ok(_tx_res) => Ok(queue_items),
            Err(err) => {
                error!("Error enqueueing token {:#?} {:#?}", &token_ids, err);
                Err(QueueError::FailedToEnqueue)
            }
        }
    }

    async fn get_batch(&self) -> Result<Vec<QueueItem>, QueueError> {
        Err(QueueError::FailedToGetBatch)
    }

    async fn get_customer_migration_state(
        &self,
        keplr_wallet_pubkey: &str,
        project_id: &str,
    ) -> Vec<QueueItem> {
        let client = self.connection_pool.get().await.unwrap();
        let rows = match client
            .query(
                "SELECT * FROM migration_queue WHERE keplr_wallet_pubkey = $1 AND project_id = $2;",
                &[&keplr_wallet_pubkey, &project_id],
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                error!("Error while fetching customer migration state : {:#?}", err);
                return Vec::new();
            }
        };

        let mut queue_items = Vec::new();
        for row in rows {
            let tx_hash: Option<String> = row.get(4);
            queue_items.push(QueueItem {
                keplr_wallet_pubkey: row.get::<usize, String>(1).into(),
                project_id: row.get::<usize, String>(2).into(),
                token_id: row.get::<usize, String>(3).into(),
                transaction_hash: tx_hash,
                status: QueueStatus::from(row.get::<usize, PostgresQueueStatus>(5)),
            });
        }
        queue_items
    }
}

impl PostgresQueueManager {
    pub fn new(connection_pool: Arc<Pool>, batch_size: u8) -> Self {
        Self {
            connection_pool,
            batch_size,
        }
    }
}
