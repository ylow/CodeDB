use std::marker::PhantomData;
use std::os::raw::c_int;
use std::sync::Arc;

use rusqlite::ffi;
use rusqlite::types::Null;
use rusqlite::vtab::{
    eponymous_only_module, Context, IndexConstraintOp, IndexInfo, VTab, VTabCursor, Values,
};

use tantivy::collector::TopDocs;
use tantivy::schema::OwnedValue;
use tantivy::snippet::SnippetGenerator;
use tantivy::{Index, IndexReader, TantivyDocument};

use crate::query_builder::build_query;
use crate::types::{ColumnDef, ColumnSource};
use crate::vtab_helpers::*;

pub struct VTabState {
    pub index: Index,
    pub reader: IndexReader,
    pub search_fields: Vec<tantivy::schema::Field>,
    pub columns: Vec<ColumnDef>,
    pub ddl: String,
    pub default_limit: usize,
}

impl VTabState {
    pub fn query_col(&self) -> i32 {
        self.columns.len() as i32
    }
    pub fn mode_col(&self) -> i32 {
        self.columns.len() as i32 + 1
    }
    pub fn limit_col(&self) -> i32 {
        self.columns.len() as i32 + 2
    }
}

#[repr(C)]
pub struct TantivyTable {
    base: ffi::sqlite3_vtab,
    state: Arc<VTabState>,
}

struct SearchResult {
    score: f32,
    field_values: Vec<Option<OwnedValue>>,
    snippet: Option<String>,
}

#[repr(C)]
pub struct TantivyCursor<'vtab> {
    base: ffi::sqlite3_vtab_cursor,
    state: Arc<VTabState>,
    results: Vec<SearchResult>,
    pos: usize,
    phantom: PhantomData<&'vtab TantivyTable>,
}

unsafe impl<'vtab> VTab<'vtab> for TantivyTable {
    type Aux = Arc<VTabState>;
    type Cursor = TantivyCursor<'vtab>;

    fn connect(
        _db: &mut rusqlite::vtab::VTabConnection,
        aux: Option<&Arc<VTabState>>,
        _args: &[&[u8]],
    ) -> rusqlite::Result<(String, Self)> {
        let state = aux.expect("VTabState aux data must be provided").clone();
        let ddl = state.ddl.clone();
        Ok((
            ddl,
            TantivyTable {
                base: ffi::sqlite3_vtab::default(),
                state,
            },
        ))
    }

    fn best_index(&self, info: &mut IndexInfo) -> rusqlite::Result<()> {
        let mut idx_num: i32 = 0;
        let mut argv_index: i32 = 1;

        // First pass: collect constraint info without borrowing info mutably
        let num_constraints = info.constraints().count();
        let mut constraint_actions: Vec<Option<i32>> = vec![None; num_constraints];

        for (i, constraint) in info.constraints().enumerate() {
            if !constraint.is_usable() {
                continue;
            }

            let col = constraint.column();
            let op = constraint.operator();

            if col == self.state.query_col()
                && op == IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ
            {
                idx_num |= IDX_QUERY;
                constraint_actions[i] = Some(argv_index);
                argv_index += 1;
            } else if col == self.state.mode_col()
                && op == IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ
            {
                idx_num |= IDX_MODE;
                constraint_actions[i] = Some(argv_index);
                argv_index += 1;
            } else if col == self.state.limit_col()
                && op == IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ
            {
                idx_num |= IDX_LIMIT_COL;
                constraint_actions[i] = Some(argv_index);
                argv_index += 1;
            }
        }

        // Second pass: apply constraint usages
        for (i, action) in constraint_actions.iter().enumerate() {
            if let Some(arg_idx) = action {
                let mut usage = info.constraint_usage(i);
                usage.set_argv_index(*arg_idx);
                usage.set_omit(true);
            }
        }

        if idx_num & IDX_QUERY != 0 {
            info.set_estimated_cost(100.0);
            info.set_estimated_rows(100);
        } else {
            info.set_estimated_cost(1e18);
            info.set_estimated_rows(i64::MAX);
        }

        info.set_idx_num(idx_num);
        Ok(())
    }

    fn open(&'vtab mut self) -> rusqlite::Result<TantivyCursor<'vtab>> {
        Ok(TantivyCursor {
            base: ffi::sqlite3_vtab_cursor::default(),
            state: self.state.clone(),
            results: Vec::new(),
            pos: 0,
            phantom: PhantomData,
        })
    }
}

unsafe impl VTabCursor for TantivyCursor<'_> {
    fn filter(
        &mut self,
        idx_num: c_int,
        _idx_str: Option<&str>,
        args: &Values<'_>,
    ) -> rusqlite::Result<()> {
        self.results.clear();
        self.pos = 0;

        let flags = decode_idx_num(idx_num);
        if !flags.has_query {
            return Ok(());
        }

        let mut arg_idx = 0;

        let query_str: String = args.get(arg_idx)?;
        arg_idx += 1;

        let mode: String = if flags.has_mode {
            let m: String = args.get(arg_idx)?;
            arg_idx += 1;
            m
        } else {
            "default".to_string()
        };

        let limit: usize = if flags.has_limit_col {
            let l: i64 = args.get(arg_idx)?;
            l.max(0) as usize
        } else {
            self.state.default_limit
        };

        let query = build_query(
            &self.state.index,
            &self.state.search_fields,
            &query_str,
            &mode,
        )
        .map_err(|e| rusqlite::Error::ModuleError(e.to_string()))?;

        let searcher = self.state.reader.searcher();
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| rusqlite::Error::ModuleError(e.to_string()))?;

        // Build snippet generator if any column needs it
        let snippet_field = self.state.columns.iter().find_map(|c| match &c.source {
            ColumnSource::Snippet(f) => Some(*f),
            _ => None,
        });
        let snippet_gen = snippet_field.and_then(|field| {
            SnippetGenerator::create(&searcher, &*query, field).ok()
        });

        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| rusqlite::Error::ModuleError(e.to_string()))?;

            let field_values: Vec<Option<OwnedValue>> = self
                .state
                .columns
                .iter()
                .map(|col| match &col.source {
                    ColumnSource::StoredField(field) => {
                        doc.get_first(*field).cloned()
                    }
                    _ => None,
                })
                .collect();

            let snippet = snippet_gen
                .as_ref()
                .map(|gen| gen.snippet_from_doc(&doc).to_html());

            self.results.push(SearchResult {
                score,
                field_values,
                snippet,
            });
        }

        Ok(())
    }

    fn next(&mut self) -> rusqlite::Result<()> {
        self.pos += 1;
        Ok(())
    }

    fn eof(&self) -> bool {
        self.pos >= self.results.len()
    }

    fn column(&self, ctx: &mut Context, col: c_int) -> rusqlite::Result<()> {
        let col_idx = col as usize;
        let result = &self.results[self.pos];

        // Hidden columns return NULL
        if col_idx >= self.state.columns.len() {
            ctx.set_result(&Null)?;
            return Ok(());
        }

        let col_def = &self.state.columns[col_idx];
        match &col_def.source {
            ColumnSource::Score => {
                ctx.set_result(&(result.score as f64))?;
            }
            ColumnSource::Snippet(_) => match &result.snippet {
                Some(s) => ctx.set_result(&s.as_str())?,
                None => ctx.set_result(&Null)?,
            },
            ColumnSource::StoredField(_) => match &result.field_values[col_idx] {
                Some(val) => set_owned_value(ctx, val)?,
                None => ctx.set_result(&Null)?,
            },
        }
        Ok(())
    }

    fn rowid(&self) -> rusqlite::Result<i64> {
        Ok(self.pos as i64)
    }
}

fn set_owned_value(ctx: &mut Context, val: &OwnedValue) -> rusqlite::Result<()> {
    match val {
        OwnedValue::Str(s) => ctx.set_result(&s.as_str())?,
        OwnedValue::U64(n) => ctx.set_result(&(*n as i64))?,
        OwnedValue::I64(n) => ctx.set_result(n)?,
        OwnedValue::F64(n) => ctx.set_result(n)?,
        OwnedValue::Bool(b) => ctx.set_result(&(*b as i32))?,
        OwnedValue::Bytes(b) => ctx.set_result(&b.as_slice())?,
        _ => ctx.set_result(&Null)?,
    }
    Ok(())
}

/// Register a validated vtab with a SQLite connection.
pub fn register_vtab(
    conn: &rusqlite::Connection,
    name: &str,
    state: Arc<VTabState>,
) -> Result<(), rusqlite::Error> {
    conn.create_module(name, eponymous_only_module::<TantivyTable>(), Some(state))
}
