use futures_async_stream::try_stream;
use crate::execution::executor::{BoxedExecutor, Executor};
use crate::execution::ExecutorError;
use crate::planner::operator::create_table::CreateTableOperator;
use crate::storage::Storage;
use crate::types::tuple::Tuple;

pub struct CreateTable {
    op: CreateTableOperator
}

impl From<CreateTableOperator> for CreateTable {
    fn from(op: CreateTableOperator) -> Self {
        CreateTable {
            op
        }
    }
}

impl<S: Storage> Executor<S> for CreateTable {
    fn execute(self, storage: &S) -> BoxedExecutor {
        self._execute(storage.clone())
    }
}

impl CreateTable {
    #[try_stream(boxed, ok = Tuple, error = ExecutorError)]
    pub async fn _execute<S: Storage>(self, storage: S) {
        let CreateTableOperator { table_name, columns } = self.op;

        let _ = storage.create_table(table_name, columns).await?;
    }
}