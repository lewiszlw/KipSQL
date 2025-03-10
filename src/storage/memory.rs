use std::cell::Cell;
use std::fmt::{Debug, Formatter};
use std::slice;
use std::sync::Arc;
use async_trait::async_trait;
use crate::catalog::{ColumnCatalog, RootCatalog, TableCatalog, TableName};
use crate::expression::simplify::ConstantBinary;
use crate::storage::{Bounds, Projections, Storage, StorageError, Transaction, Iter, tuple_projection, IndexIter};
use crate::types::index::{Index, IndexMetaRef};
use crate::types::tuple::{Tuple, TupleId};

// WARRING: Only single-threaded and tested using
#[derive(Clone)]
pub struct MemStorage {
    inner: Arc<Cell<StorageInner>>
}

unsafe impl Send for MemStorage {

}

unsafe impl Sync for MemStorage {

}

impl MemStorage {
    pub fn new() -> MemStorage {
        Self {
            inner: Arc::new(
                Cell::new(
                    StorageInner {
                        root: Default::default(),
                        tables: Default::default(),
                    }
                )
            ),
        }
    }

    pub fn root(self, root: RootCatalog) -> Self {
        unsafe {
            self.inner.as_ptr().as_mut().unwrap().root = root;
        }
        self
    }
}

#[derive(Debug)]
struct StorageInner {
    root: RootCatalog,
    tables: Vec<(TableName, MemTable)>
}

#[async_trait]
impl Storage for MemStorage {
    type TransactionType = MemTable;

    async fn create_table(&self, table_name: TableName, columns: Vec<ColumnCatalog>) -> Result<TableName, StorageError> {
        let new_table = MemTable {
            tuples: Arc::new(Cell::new(vec![])),
        };
        let inner = unsafe { self.inner.as_ptr().as_mut() }.unwrap();

        let table_id = inner.root.add_table(table_name.clone(), columns)?;
        inner.tables.push((table_name, new_table));

        Ok(table_id)
    }

    async fn drop_table(&self, name: &String) -> Result<(), StorageError> {
        let inner = unsafe {
            self.inner
                .as_ptr()
                .as_mut()
                .unwrap()
        };

        inner.root.drop_table(&name)?;

        Ok(())
    }

    async fn drop_data(&self, name: &String) -> Result<(), StorageError> {
        let inner = unsafe {
            self.inner
                .as_ptr()
                .as_mut()
                .unwrap()
        };

        inner.tables.retain(|(t_name, _)| t_name.as_str() != name);

        Ok(())
    }

    async fn transaction(&self, name: &String) -> Option<Self::TransactionType> {
        unsafe {
            self.inner
                .as_ptr()
                .as_ref()
                .unwrap()
                .tables
                .iter()
                .find(|(tname, _)| tname.as_str() == name)
                .map(|(_, table)| table.clone())
        }
    }

    async fn table(&self, name: &String) -> Option<&TableCatalog> {
        unsafe {
            self.inner
                .as_ptr()
                .as_ref()
                .unwrap()
                .root
                .get_table(name)
        }
    }

    async fn show_tables(&self) -> Result<Vec<String>, StorageError> {
        todo!()
    }
}

unsafe impl Send for MemTable {

}

unsafe impl Sync for MemTable {

}

#[derive(Clone)]
pub struct MemTable {
    tuples: Arc<Cell<Vec<Tuple>>>
}

impl Debug for MemTable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        unsafe {
            f.debug_struct("MemTable")
                .field("{:?}", self.tuples.as_ptr().as_ref().unwrap())
                .finish()
        }
    }
}

#[async_trait]
impl Transaction for MemTable {
    type IterType<'a> = MemTraction<'a>;

    fn read(&self, bounds: Bounds, projection: Projections) -> Result<Self::IterType<'_>, StorageError> {
        unsafe {
            Ok(
                MemTraction {
                    offset: bounds.0.unwrap_or(0),
                    limit: bounds.1,
                    projections: projection,
                    iter: self.tuples.as_ptr().as_ref().unwrap().iter(),
                }
            )
        }
    }

    #[allow(unused_variables)]
    fn read_by_index(&self, bounds: Bounds, projection: Projections, index_meta: IndexMetaRef, binaries: Vec<ConstantBinary>) -> Result<IndexIter<'_>, StorageError> {
        todo!()
    }

    #[allow(unused_variables)]
    fn add_index(&mut self, index: Index, tuple_ids: Vec<TupleId>, is_unique: bool) -> Result<(), StorageError> {
        todo!()
    }

    fn del_index(&mut self, _index: &Index) -> Result<(), StorageError> {
        todo!()
    }

    fn append(&mut self, tuple: Tuple, is_overwrite: bool) -> Result<(), StorageError> {
        let tuples = unsafe {
            self.tuples
                .as_ptr()
                .as_mut()
        }.unwrap();

        if let Some(original_tuple) = tuples.iter_mut().find(|t| t.id == tuple.id) {
            if !is_overwrite {
                return Err(StorageError::DuplicatePrimaryKey);
            }
            *original_tuple = tuple;
        } else {
            tuples.push(tuple);
        }

        Ok(())
    }

    fn delete(&mut self, tuple_id: TupleId) -> Result<(), StorageError> {
        let tuples = unsafe {
            self.tuples
                .as_ptr()
                .as_mut()
        }.unwrap();

        tuples.retain(|tuple| tuple.id.clone().unwrap() != tuple_id);

        Ok(())
    }

    async fn commit(self) -> Result<(), StorageError> {
        Ok(())
    }
}

pub struct MemTraction<'a> {
    offset: usize,
    limit: Option<usize>,
    projections: Projections,
    iter: slice::Iter<'a, Tuple>
}

impl Iter for MemTraction<'_> {
    fn next_tuple(&mut self) -> Result<Option<Tuple>, StorageError> {
        while self.offset > 0 {
            let _ = self.iter.next();
            self.offset -= 1;
        }

        if let Some(num) = self.limit {
            if num == 0 {
                return Ok(None);
            }
        }

        self.iter
            .next()
            .cloned()
            .map(|tuple| tuple_projection(&mut self.limit, &self.projections, tuple))
            .transpose()
    }
}

#[cfg(test)]
pub(crate) mod test {
    use std::sync::Arc;
    use itertools::Itertools;
    use crate::catalog::{ColumnCatalog, ColumnDesc, ColumnRef};
    use crate::expression::ScalarExpression;
    use crate::storage::memory::MemStorage;
    use crate::storage::{Storage, StorageError, Transaction, Iter};
    use crate::types::LogicalType;
    use crate::types::tuple::Tuple;
    use crate::types::value::DataValue;

    pub fn data_filling(columns: Vec<ColumnRef>, table: &mut impl Transaction) -> Result<(), StorageError> {
        table.append(Tuple {
            id: Some(Arc::new(DataValue::Int32(Some(1)))),
            columns: columns.clone(),
            values: vec![
                Arc::new(DataValue::Int32(Some(1))),
                Arc::new(DataValue::Boolean(Some(true)))
            ],
        }, false)?;
        table.append(Tuple {
            id: Some(Arc::new(DataValue::Int32(Some(2)))),
            columns: columns.clone(),
            values: vec![
                Arc::new(DataValue::Int32(Some(2))),
                Arc::new(DataValue::Boolean(Some(false)))
            ],
        }, false)?;

        Ok(())
    }

    #[tokio::test]
    async fn test_in_memory_storage_works_with_data() -> Result<(), StorageError> {
        let storage = MemStorage::new();
        let columns = vec![
            Arc::new(ColumnCatalog::new(
                "c1".to_string(),
                false,
                ColumnDesc::new(LogicalType::Integer, true, false),
                None
            )),
            Arc::new(ColumnCatalog::new(
                "c2".to_string(),
                false,
                ColumnDesc::new(LogicalType::Boolean, false, false),
                None
            )),
        ];

        let source_columns = columns.iter()
            .map(|col_ref| ColumnCatalog::clone(&col_ref))
            .collect_vec();

        let table_id = storage.create_table(Arc::new("test".to_string()), source_columns).await?;

        let table_catalog = storage.table(&"test".to_string()).await;
        assert!(table_catalog.is_some());
        assert!(table_catalog.unwrap().get_column_id_by_name(&"c1".to_string()).is_some());

        let mut transaction = storage.transaction(&table_id).await.unwrap();
        data_filling(columns, &mut transaction)?;

        let mut iter = transaction.read(
            (Some(1), Some(1)),
            vec![ScalarExpression::InputRef { index: 0, ty: LogicalType::Integer }]
        )?;

        let option_1 = iter.next_tuple()?;
        assert_eq!(option_1.unwrap().id, Some(Arc::new(DataValue::Int32(Some(2)))));

        let option_2 = iter.next_tuple()?;
        assert_eq!(option_2, None);

        Ok(())
    }
}