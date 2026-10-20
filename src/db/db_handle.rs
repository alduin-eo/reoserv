use std::sync::Arc;

use tokio::sync::{
    Mutex, OwnedMutexGuard,
    mpsc::{self, UnboundedSender},
};

use super::{Command, Connection, Db, IntoDbRequest};

#[derive(Debug, Clone)]
pub struct DbHandle {
    tx: UnboundedSender<Command>,
    // All maps share this one connection, so transactions must not interleave:
    // a second START TRANSACTION would implicitly commit the first one.
    transaction_lock: Arc<Mutex<()>>,
}

impl DbHandle {
    pub fn new(connection: Connection) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let db = Db::new(rx, connection);
        tokio::spawn(run_db(db));
        Self {
            tx,
            transaction_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Hold the returned guard from start_transaction until commit/rollback.
    pub async fn lock_transactions(&self) -> OwnedMutexGuard<()> {
        self.transaction_lock.clone().lock_owned().await
    }

    pub async fn execute<Q>(&self, query: &Q) -> anyhow::Result<()>
    where
        Q: IntoDbRequest + ?Sized,
    {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let _ = self
            .tx
            .send(Command::Execute(query.to_db_request(), resp_tx));
        resp_rx.await?
    }

    pub async fn query<Q>(&self, query: &Q) -> anyhow::Result<Vec<crate::db::Row>>
    where
        Q: IntoDbRequest + ?Sized,
    {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(Command::Query(query.to_db_request(), resp_tx));
        resp_rx.await?
    }

    pub async fn query_map<Q, T, F>(&self, query: &Q, map_fn: F) -> anyhow::Result<Vec<T>>
    where
        Q: IntoDbRequest + ?Sized,
        F: Fn(crate::db::Row) -> T,
    {
        let rows = self.query(query).await?;
        Ok(rows.into_iter().map(map_fn).collect())
    }

    pub async fn try_query_map<Q, T, F>(&self, query: &Q, map_fn: F) -> anyhow::Result<Vec<T>>
    where
        Q: IntoDbRequest + ?Sized,
        F: Fn(crate::db::Row) -> anyhow::Result<T>,
    {
        let rows = self.query(query).await?;
        rows.into_iter()
            .map(map_fn)
            .collect::<anyhow::Result<Vec<T>>>()
    }

    pub async fn query_one<Q>(&self, query: &Q) -> anyhow::Result<Option<crate::db::Row>>
    where
        Q: IntoDbRequest + ?Sized,
    {
        let rows = self.query(query).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn query_string<Q>(&self, query: &Q) -> anyhow::Result<Option<String>>
    where
        Q: IntoDbRequest + ?Sized,
    {
        let row = self.query_one(query).await?;
        Ok(row.and_then(|r| r.get_string(0)))
    }

    pub async fn query_int<Q>(&self, query: &Q) -> anyhow::Result<Option<i32>>
    where
        Q: IntoDbRequest + ?Sized,
    {
        let row = self.query_one(query).await?;
        Ok(row.and_then(|r| r.get_int(0)))
    }

    #[allow(dead_code)]
    pub async fn query_date<Q>(&self, query: &Q) -> anyhow::Result<Option<chrono::NaiveDateTime>>
    where
        Q: IntoDbRequest + ?Sized,
    {
        let row = self.query_one(query).await?;
        Ok(row.and_then(|r| r.get_date(0)))
    }

    pub async fn start_transaction(&self) -> anyhow::Result<()> {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(Command::StartTransaction(resp_tx));
        resp_rx.await?
    }

    pub async fn commit_transaction(&self) -> anyhow::Result<()> {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(Command::CommitTransaction(resp_tx));
        resp_rx.await?
    }

    pub async fn get_last_insert_id(&self) -> Option<u64> {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(Command::GetLastInsertId(resp_tx));
        resp_rx.await.ok().flatten()
    }

    pub async fn rollback_transaction(&self) -> anyhow::Result<()> {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(Command::RollbackTransaction(resp_tx));
        resp_rx.await?
    }
}

async fn run_db(mut db: Db) {
    loop {
        if let Some(command) = db.rx.recv().await {
            db.handle_command(command).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn save(db: DbHandle, id: i32) -> anyhow::Result<()> {
        let _guard = db.lock_transactions().await;
        db.start_transaction().await?;
        db.execute(&format!("INSERT INTO test (id) VALUES ({})", id))
            .await?;
        tokio::task::yield_now().await;
        db.execute(&format!("INSERT INTO test (id) VALUES ({})", id + 1000))
            .await?;
        db.commit_transaction().await
    }

    #[tokio::test]
    async fn test_concurrent_transactions_do_not_interleave() {
        let connection = Connection::Sqlite(rusqlite::Connection::open_in_memory().unwrap());
        let db = DbHandle::new(connection);
        db.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();

        let tasks: Vec<_> = (1..=8)
            .map(|id| tokio::spawn(save(db.clone(), id)))
            .collect();
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let count = db.query_int("SELECT COUNT(1) FROM test").await.unwrap();
        assert_eq!(count, Some(16));
    }
}
