use std::time::Instant;
use sqllogictest::{AsyncDB, DBOutput, DefaultColumnType};
use kip_sql::db::{Database, DatabaseError};
use kip_sql::storage::kip::KipStorage;

pub struct KipSQL {
    pub db: Database<KipStorage>,
}

#[async_trait::async_trait]
impl AsyncDB for KipSQL {
    type Error = DatabaseError;
    type ColumnType = DefaultColumnType;

    async fn run(&mut self, sql: &str) -> Result<DBOutput<Self::ColumnType>, Self::Error> {
        let start = Instant::now();
        let tuples = self.db.run(sql).await?;
        println!("|— Input SQL:");
        println!(" |— {}", sql);
        println!(" |— Time consuming: {:?}", start.elapsed());

        if tuples.is_empty() {
            return Ok(DBOutput::StatementComplete(0));
        }

        let types = vec![DefaultColumnType::Any; tuples[0].columns.len()];
        let rows = tuples
            .into_iter()
            .map(|tuple| {
                tuple
                    .values
                    .into_iter()
                    .map(|value| format!("{}", value))
                    .collect()
            })
            .collect();
        Ok(DBOutput::Rows { types, rows })
    }
}
