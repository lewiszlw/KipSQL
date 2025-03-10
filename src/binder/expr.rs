use crate::binder::BindError;
use itertools::Itertools;
use sqlparser::ast::{BinaryOperator, Expr, Function, FunctionArg, FunctionArgExpr, Ident, UnaryOperator};
use std::slice;
use std::sync::Arc;
use async_recursion::async_recursion;
use crate::expression::agg::AggKind;

use super::Binder;
use crate::expression::ScalarExpression;
use crate::storage::Storage;
use crate::types::LogicalType;
use crate::types::value::DataValue;

impl<S: Storage> Binder<S> {
    #[async_recursion]
    pub(crate) async fn bind_expr(&mut self, expr: &Expr) -> Result<ScalarExpression, BindError> {
        match expr {
            Expr::Identifier(ident) => {
                self.bind_column_ref_from_identifiers(slice::from_ref(ident), None).await
            }
            Expr::CompoundIdentifier(idents) => {
                self.bind_column_ref_from_identifiers(idents, None).await
            }
            Expr::BinaryOp { left, right, op} => {
                self.bind_binary_op_internal(left, right, op).await
            }
            Expr::Value(v) => Ok(ScalarExpression::Constant(Arc::new(v.into()))),
            Expr::Function(func) => self.bind_agg_call(func).await,
            Expr::Nested(expr) => self.bind_expr(expr).await,
            Expr::UnaryOp { expr, op } => self.bind_unary_op_internal(expr, op).await,
            _ => {
                todo!()
            }
        }
    }

    pub async fn bind_column_ref_from_identifiers(
        &mut self,
        idents: &[Ident],
        bind_table_name: Option<&String>,
    ) -> Result<ScalarExpression, BindError> {
        let idents = idents
            .iter()
            .map(|ident| Ident::new(ident.value.to_lowercase()))
            .collect_vec();
        let (_schema_name, table_name, column_name) = match idents.as_slice() {
            [column] => (None, None, &column.value),
            [table, column] => (None, Some(&table.value), &column.value),
            [schema, table, column] => (Some(&schema.value), Some(&table.value), &column.value),
            _ => {
                return Err(BindError::InvalidColumn(
                    idents
                        .iter()
                        .map(|ident| ident.value.clone())
                        .join(".")
                        .to_string(),
                )
                .into())
            }
        };

        if let Some(table) = table_name.or(bind_table_name) {
            let table_catalog = self
                .context
                .storage
                .table(table)
                .await
                .ok_or_else(|| BindError::InvalidTable(table.to_string()))?;

            let column_catalog = table_catalog
                .get_column_by_name(column_name)
                .ok_or_else(|| BindError::InvalidColumn(column_name.to_string()))?;
            Ok(ScalarExpression::ColumnRef(column_catalog.clone()))
        } else {
            // handle col syntax
            let mut got_column = None;
            for (_, (table_catalog, _)) in &self.context.bind_table {
                if let Some(column_catalog) = table_catalog.get_column_by_name(column_name) {
                    if got_column.is_some() {
                        return Err(BindError::InvalidColumn(column_name.to_string()).into());
                    }
                    got_column = Some(column_catalog);
                }
            }
            if got_column.is_none() {
                if let Some(expr) = self.context.aliases.get(column_name) {
                    return Ok(ScalarExpression::Alias { expr: Box::new(expr.clone()), alias: column_name.clone() });
                }
            }
            let column_catalog =
                got_column.ok_or_else(|| BindError::InvalidColumn(column_name.to_string()))?;
            Ok(ScalarExpression::ColumnRef(column_catalog.clone()))
        }
    }

    async fn bind_binary_op_internal(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: &BinaryOperator,
    ) -> Result<ScalarExpression, BindError> {
        let left_expr = Box::new(self.bind_expr(left).await?);
        let right_expr = Box::new(self.bind_expr(right).await?);

        let ty = match op {
            BinaryOperator::Plus | BinaryOperator::Minus | BinaryOperator::Multiply |
            BinaryOperator::Divide | BinaryOperator::Modulo => {
                LogicalType::max_logical_type(
                    &left_expr.return_type(),
                    &right_expr.return_type()
                )?
            }
            BinaryOperator::Gt | BinaryOperator::Lt | BinaryOperator::GtEq |
            BinaryOperator::LtEq | BinaryOperator::Eq | BinaryOperator::NotEq |
            BinaryOperator::And | BinaryOperator::Or | BinaryOperator::Xor => {
                LogicalType::Boolean
            },
            _ => todo!()
        };

        Ok(ScalarExpression::Binary {
            op: (op.clone()).into(),
            left_expr,
            right_expr,
            ty,
        })
    }

    async fn bind_unary_op_internal(
        &mut self,
        expr: &Expr,
        op: &UnaryOperator,
    ) -> Result<ScalarExpression, BindError> {
        let expr = Box::new(self.bind_expr(expr).await?);
        let ty = if let UnaryOperator::Not = op {
            LogicalType::Boolean
        } else {
            expr.return_type()
        };

        Ok(ScalarExpression::Unary {
            op: (op.clone()).into(),
            expr,
            ty,
        })
    }

    async fn bind_agg_call(&mut self, func: &Function) -> Result<ScalarExpression, BindError> {
        let mut args = Vec::with_capacity(func.args.len());

        for arg in func.args.iter() {
            let arg_expr = match arg {
                FunctionArg::Named { arg, .. } => arg,
                FunctionArg::Unnamed(arg) => arg,
            };
            match arg_expr {
                FunctionArgExpr::Expr(expr) => args.push(self.bind_expr(expr).await?),
                FunctionArgExpr::Wildcard => args.push(Self::wildcard_expr()),
                _ => todo!()
            }
        }
        let ty = args[0].return_type();

        Ok(match func.name.to_string().to_lowercase().as_str() {
            "count" => ScalarExpression::AggCall{
                distinct: func.distinct,
                kind: AggKind::Count,
                args,
                ty: LogicalType::Integer,
            },
            "sum" => ScalarExpression::AggCall{
                distinct: func.distinct,
                kind: AggKind::Sum,
                args,
                ty,
            },
            "min" => ScalarExpression::AggCall{
                distinct: func.distinct,
                kind: AggKind::Min,
                args,
                ty,
            },
            "max" => ScalarExpression::AggCall{
                distinct: func.distinct,
                kind: AggKind::Max,
                args,
                ty,
            },
            "avg" => ScalarExpression::AggCall{
                distinct: func.distinct,
                kind: AggKind::Avg,
                args,
                ty,
            },
            _ => todo!(),
        })
    }

    fn wildcard_expr() -> ScalarExpression {
        ScalarExpression::Constant(Arc::new(DataValue::Utf8(Some("*".to_string()))))
    }
}
