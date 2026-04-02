//! Native Monty extension: `datatools` using the module-level macro API.
//!
//! This example demonstrates the declarative, PyO3-inspired authoring experience
//! where extension authors think in modules, classes, functions, and methods.
//! The `#[monty_module]` macro on the `mod` block generates all ABI boilerplate.

use std::collections::HashMap;

use monty_extension_api::{ExtError, ExtValue, ExtValueResult, TryIntoExtValue, monty_module};

/// Cell values stored inside the demo dataframe.
#[derive(Clone, Debug)]
enum CellValue {
    /// A numeric value parsed from CSV.
    Float(f64),
    /// A textual value.
    Text(String),
}

impl CellValue {
    /// Returns the numeric value when this cell is numeric.
    fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Text(_) => None,
        }
    }
}

impl TryIntoExtValue for CellValue {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        match self {
            Self::Float(value) => Ok(ExtValue::Float(value)),
            Self::Text(value) => value.try_into_ext_value(),
        }
    }
}

#[monty_module(
    name = "datatools",
    version = "0.1.0",
    skill = SKILL_TEXT,
    stubs = TYPE_STUB
)]
mod datatools_ext {
    use super::*;

    /// A simple in-memory DataFrame with column-oriented storage.
    ///
    /// This intentionally keeps the data model small so the example can focus on
    /// the extension API rather than dataframe implementation details.
    #[monty_class]
    struct DataFrame {
        /// Column names in order.
        columns: Vec<String>,
        /// Column data keyed by column name.
        data: HashMap<String, Vec<CellValue>>,
        /// Number of rows.
        row_count: usize,
    }

    /// Parses CSV text into a dataframe handle.
    #[monty_function()]
    fn parse_csv(ext: &Extension, text: &str) -> Result<DataFrameHandle, ExtError> {
        let mut lines = text.lines();
        let header_line = lines
            .next()
            .ok_or_else(|| ExtError::value_error("CSV text is empty"))?;

        let columns: Vec<String> = header_line
            .split(',')
            .map(|column| column.trim().to_string())
            .collect();
        let mut data: HashMap<String, Vec<CellValue>> = HashMap::new();
        for column in &columns {
            data.insert(column.clone(), Vec::new());
        }

        let mut row_count = 0;
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let fields: Vec<&str> = line.split(',').collect();
            for (index, column) in columns.iter().enumerate() {
                let raw = fields.get(index).map_or("", |field| field.trim());
                let cell = match raw.parse::<f64>() {
                    Ok(value) => CellValue::Float(value),
                    Err(_) => CellValue::Text(raw.to_string()),
                };
                data.get_mut(column).expect("column exists").push(cell);
            }
            row_count += 1;
        }

        Ok(ext.store_data_frame(DataFrame {
            columns,
            data,
            row_count,
        }))
    }

    /// Returns the dataframe row count as a top-level function.
    #[monty_function(name = "row_count")]
    fn row_count_function(
        ext: &Extension,
        df: DataFrameHandle,
    ) -> Result<usize, ExtError> {
        row_count_impl(ext, &df)
    }

    /// Returns the dataframe column names as a top-level function.
    #[monty_function(name = "columns")]
    fn columns_function(
        ext: &Extension,
        df: DataFrameHandle,
    ) -> Result<Vec<String>, ExtError> {
        columns_impl(ext, &df)
    }

    /// Returns the first `n` rows as dictionaries.
    #[monty_function(name = "head")]
    fn head_function(
        ext: &Extension,
        df: DataFrameHandle,
        n: Option<usize>,
    ) -> Result<Vec<HashMap<String, CellValue>>, ExtError> {
        head_impl(ext, &df, n)
    }

    /// Sums a numeric column as a top-level function.
    #[monty_function(name = "column_sum")]
    fn column_sum_function(
        ext: &Extension,
        df: DataFrameHandle,
        col: &str,
    ) -> Result<f64, ExtError> {
        column_sum_impl(ext, &df, col)
    }

    /// Computes the numeric column mean as a top-level function.
    #[monty_function(name = "column_mean")]
    fn column_mean_function(
        ext: &Extension,
        df: DataFrameHandle,
        col: &str,
    ) -> Result<f64, ExtError> {
        column_mean_impl(ext, &df, col)
    }

    /// Filters rows by a numeric threshold as a top-level function.
    #[monty_function(name = "filter_gt")]
    fn filter_gt_function(
        ext: &Extension,
        df: DataFrameHandle,
        col: &str,
        threshold: f64,
    ) -> Result<DataFrameHandle, ExtError> {
        filter_gt_impl(ext, &df, col, threshold)
    }

    /// DataFrame handle methods.
    #[monty_methods]
    impl DataFrame {
        #[monty_method(name = "row_count")]
        fn row_count_method(
            ext: &Extension,
            df: DataFrameHandle,
        ) -> Result<usize, ExtError> {
            row_count_impl(ext, &df)
        }

        #[monty_method(name = "columns")]
        fn columns_method(
            ext: &Extension,
            df: DataFrameHandle,
        ) -> Result<Vec<String>, ExtError> {
            columns_impl(ext, &df)
        }

        #[monty_method(name = "head")]
        fn head_method(
            ext: &Extension,
            df: DataFrameHandle,
            n: Option<usize>,
        ) -> Result<Vec<HashMap<String, CellValue>>, ExtError> {
            head_impl(ext, &df, n)
        }

        #[monty_method(name = "column_sum")]
        fn column_sum_method(
            ext: &Extension,
            df: DataFrameHandle,
            col: &str,
        ) -> Result<f64, ExtError> {
            column_sum_impl(ext, &df, col)
        }

        #[monty_method(name = "column_mean")]
        fn column_mean_method(
            ext: &Extension,
            df: DataFrameHandle,
            col: &str,
        ) -> Result<f64, ExtError> {
            column_mean_impl(ext, &df, col)
        }

        #[monty_method(name = "filter_gt")]
        fn filter_gt_method(
            ext: &Extension,
            df: DataFrameHandle,
            col: &str,
            threshold: f64,
        ) -> Result<DataFrameHandle, ExtError> {
            filter_gt_impl(ext, &df, col, threshold)
        }
    }

    /// Clears all live handles when the extension unloads.
    #[monty_shutdown()]
    fn shutdown_extension(ext: &Extension) {
        ext.objects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    // ─── Shared implementation helpers ───────────────────────────────────

    fn row_count_impl(ext: &Extension, df: &DataFrameHandle) -> Result<usize, ExtError> {
        ext.with_data_frame(df, "row_count", |frame| Ok(frame.row_count))
    }

    fn columns_impl(ext: &Extension, df: &DataFrameHandle) -> Result<Vec<String>, ExtError> {
        ext.with_data_frame(df, "columns", |frame| Ok(frame.columns.clone()))
    }

    fn head_impl(
        ext: &Extension,
        df: &DataFrameHandle,
        n: Option<usize>,
    ) -> Result<Vec<HashMap<String, CellValue>>, ExtError> {
        ext.with_data_frame(df, "head", |frame| {
            let take = n.unwrap_or(5).min(frame.row_count);
            let mut rows = Vec::with_capacity(take);
            for row_index in 0..take {
                let mut row = HashMap::new();
                for column in &frame.columns {
                    row.insert(column.clone(), frame.data[column][row_index].clone());
                }
                rows.push(row);
            }
            Ok(rows)
        })
    }

    fn column_sum_impl(ext: &Extension, df: &DataFrameHandle, col: &str) -> Result<f64, ExtError> {
        ext.with_data_frame(df, "column_sum", |frame| {
            let column = get_column(frame, col, "column_sum")?;
            Ok(column.iter().filter_map(CellValue::as_float).sum())
        })
    }

    fn column_mean_impl(
        ext: &Extension,
        df: &DataFrameHandle,
        col: &str,
    ) -> Result<f64, ExtError> {
        ext.with_data_frame(df, "column_mean", |frame| {
            let column = get_column(frame, col, "column_mean")?;
            let values: Vec<f64> = column.iter().filter_map(CellValue::as_float).collect();
            if values.is_empty() {
                return Err(ExtError::value_error(format!(
                    "column '{col}' has no numeric values"
                )));
            }
            Ok(values.iter().sum::<f64>() / values.len() as f64)
        })
    }

    fn filter_gt_impl(
        ext: &Extension,
        df: &DataFrameHandle,
        col: &str,
        threshold: f64,
    ) -> Result<DataFrameHandle, ExtError> {
        let filtered = ext.with_data_frame(df, "filter_gt", |frame| {
            let filter_column = get_column(frame, col, "filter_gt")?;
            let mask: Vec<bool> = filter_column
                .iter()
                .map(|cell| cell.as_float().is_some_and(|value| value > threshold))
                .collect();

            let mut new_data = HashMap::new();
            for column in &frame.columns {
                let filtered_values: Vec<CellValue> = frame.data[column]
                    .iter()
                    .zip(&mask)
                    .filter(|(_, keep)| **keep)
                    .map(|(cell, _)| cell.clone())
                    .collect();
                new_data.insert(column.clone(), filtered_values);
            }

            Ok(DataFrame {
                columns: frame.columns.clone(),
                data: new_data,
                row_count: mask.into_iter().filter(|keep| *keep).count(),
            })
        })?;

        Ok(ext.store_data_frame(filtered))
    }

    /// Retrieves a named column or returns a precise `KeyError`.
    fn get_column<'a>(
        frame: &'a DataFrame,
        col: &str,
        func: &str,
    ) -> Result<&'a Vec<CellValue>, ExtError> {
        frame
            .data
            .get(col)
            .ok_or_else(|| ExtError::key_error(format!("{func}(): column '{col}' not found")))
    }
}

/// Type stub source injected into Monty's type checker.
const TYPE_STUB: &str = r"
from typing import Any

class DataFrame:
    def row_count(self) -> int: ...
    def columns(self) -> list[str]: ...
    def head(self, n: int = 5) -> list[dict[str, Any]]: ...
    def column_sum(self, col: str) -> float: ...
    def column_mean(self, col: str) -> float: ...
    def filter_gt(self, col: str, threshold: float) -> DataFrame: ...

def parse_csv(text: str) -> DataFrame: ...
def row_count(df: DataFrame) -> int: ...
def columns(df: DataFrame) -> list[str]: ...
def head(df: DataFrame, n: int = 5) -> list[dict[str, Any]]: ...
def column_sum(df: DataFrame, col: str) -> float: ...
def column_mean(df: DataFrame, col: str) -> float: ...
def filter_gt(df: DataFrame, col: str, threshold: float) -> DataFrame: ...
";

/// Skill text describing the extension for prompt injection.
const SKILL_TEXT: &str = r"# datatools -- In-Memory DataFrame Operations

You have access to `import datatools` for CSV parsing and basic data analysis.

## Available functions

- `datatools.parse_csv(text: str) -> DataFrame` -- Parse CSV text (first row = headers) into a DataFrame
- `datatools.row_count(df: DataFrame) -> int` -- Number of rows
- `datatools.columns(df: DataFrame) -> list[str]` -- Column names
- `datatools.head(df: DataFrame, n: int = 5) -> list[dict]` -- First N rows as list of dicts
- `datatools.column_sum(df: DataFrame, col: str) -> float` -- Sum of a numeric column
- `datatools.column_mean(df: DataFrame, col: str) -> float` -- Mean of a numeric column
- `datatools.filter_gt(df: DataFrame, col: str, threshold: float) -> DataFrame` -- Filter rows where column > threshold

## Patterns

- Parse once, then reuse the same DataFrame handle for multiple operations.
- Use `head()` when you need rows returned as regular Python data structures.
- `filter_gt()` produces a new DataFrame handle and does not mutate the original.
";
