use std::collections::BTreeMap;
use thiserror::Error;

use neon_ygo_core::{CardCode, CardId, DuelEngine, DuelEvent, Effect, EffectId, PlayerId};

use crate::ast::{BinOp, Expr, Stmt, TableField, UnOp};
use crate::ocg_api::{call_ocg_native, register_ocg_globals};
use crate::parser::{parse, ParseError};
use crate::value::{EnvId, FuncId, GroupId, LuaClosure, LuaEnv, LuaGroup, LuaTable, LuaVal, TableId, TableKey};

#[derive(Debug, Error, Clone)]
pub enum LuaError {
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("runtime error: {0}")]
    Runtime(String),
}

enum ControlFlow {
    None,
    Break,
    Return(Vec<LuaVal>),
}

/// Complete 100% pure-Rust Lua 5.4 VM for YGOPro / EDOPro / YGO Omega card & puzzle scripts!
#[derive(Debug, Clone)]
pub struct LuaVm {
    pub globals: TableId,
    pub tables: BTreeMap<TableId, LuaTable>,
    pub next_table_id: TableId,
    pub closures: BTreeMap<FuncId, LuaClosure>,
    pub next_func_id: FuncId,
    pub groups: BTreeMap<GroupId, LuaGroup>,
    pub next_group_id: GroupId,
    pub envs: BTreeMap<EnvId, LuaEnv>,
    pub next_env_id: EnvId,
    /// Temporary Effect builder drafts created by `Effect.CreateEffect(c)` before `c:RegisterEffect(e)`.
    pub draft_effects: BTreeMap<EffectId, Effect>,
    /// Maps draft EffectId -> registered DuelEngine EffectId so `e1:Clone()` / `e1:SetDescription()` work after registration!
    pub effect_id_aliases: BTreeMap<EffectId, EffectId>,
    pub next_draft_effect_id: EffectId,
    /// Active `GetID()` context when loading a card script.
    pub current_script_table: Option<TableId>,
    pub current_script_code: CardCode,
    /// Operated group from most recent Duel operation (`Duel.GetOperatedGroup()`).
    pub operated_group: Vec<CardId>,
    /// Puzzle hints & AI name captured from `Debug.*` API.
    pub puzzle_hints: Vec<String>,
    pub puzzle_ai_name: String,
    /// Printed messages / logs.
    pub output_log: Vec<String>,
    /// Optional card catalog so `Debug.AddCard` and `Duel.CreateToken` hydrate full card metadata.
    pub card_catalog: BTreeMap<CardCode, neon_ygo_core::CardData>,
    /// Script search directories for `Duel.LoadScript(...)` & `Duel.LoadCardScript(...)`.
    pub script_search_dirs: Vec<std::path::PathBuf>,
    /// Already-loaded helper script filenames (avoid duplicate loading in `Duel.LoadScript`).
    pub loaded_helper_scripts: std::collections::BTreeSet<String>,
    /// Per-CardId metatable / script table (`c.__index` & `c:GetMetatable()` & `getmetatable(c)` in ocgcore!).
    pub card_metatables: BTreeMap<CardId, TableId>,
}

impl Default for LuaVm {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaVm {
    pub fn new() -> Self {
        let mut vm = Self {
            globals: 1,
            tables: BTreeMap::new(),
            next_table_id: 2,
            closures: BTreeMap::new(),
            next_func_id: 1,
            groups: BTreeMap::new(),
            next_group_id: 1,
            envs: BTreeMap::new(),
            next_env_id: 1,
            draft_effects: BTreeMap::new(),
            effect_id_aliases: BTreeMap::new(),
            next_draft_effect_id: 100_000,
            current_script_table: None,
            current_script_code: 0,
            operated_group: Vec::new(),
            puzzle_hints: Vec::new(),
            puzzle_ai_name: String::new(),
            output_log: Vec::new(),
            card_catalog: BTreeMap::new(),
            script_search_dirs: Vec::new(),
            loaded_helper_scripts: std::collections::BTreeSet::new(),
            card_metatables: BTreeMap::new(),
        };
        vm.tables.insert(1, LuaTable::default());
        vm.register_stdlib();
        register_ocg_globals(&mut vm);
        vm
    }

    pub fn alloc_table(&mut self) -> TableId {
        let id = self.next_table_id;
        self.next_table_id += 1;
        self.tables.insert(id, LuaTable::default());
        id
    }

    pub fn alloc_group(&mut self, cards: Vec<CardId>) -> GroupId {
        let id = self.next_group_id;
        self.next_group_id += 1;
        self.groups.insert(id, LuaGroup::new(cards));
        id
    }

    pub fn alloc_env(&mut self, parent: Option<EnvId>) -> EnvId {
        let id = self.next_env_id;
        self.next_env_id += 1;
        self.envs.insert(id, LuaEnv { parent, vars: BTreeMap::new() });
        id
    }

    pub fn set_global(&mut self, name: &str, val: LuaVal) {
        if let Some(g) = self.tables.get_mut(&self.globals) {
            g.set(TableKey::Str(name.to_string()), val);
        }
    }

    pub fn get_global(&self, name: &str) -> LuaVal {
        self.tables
            .get(&self.globals)
            .map(|g| g.get(&TableKey::Str(name.to_string())))
            .unwrap_or(LuaVal::Nil)
    }

    pub fn set_table_field(&mut self, tid: TableId, key: &str, val: LuaVal) {
        if let Some(t) = self.tables.get_mut(&tid) {
            t.set(TableKey::Str(key.to_string()), val);
        }
    }

    pub fn get_table_val(&self, tid: TableId, key: &TableKey) -> LuaVal {
        if let Some(t) = self.tables.get(&tid) {
            let v = t.get(key);
            if !matches!(v, LuaVal::Nil) {
                return v;
            }
            if let Some(mt) = t.metatable {
                if let Some(mt_tbl) = self.tables.get(&mt) {
                    if let LuaVal::Table(idx_tid) = mt_tbl.get(&TableKey::Str("__index".into())) {
                        return self.get_table_val(idx_tid, key);
                    }
                }
            }
        }
        LuaVal::Nil
    }

    fn register_stdlib(&mut self) {
        self.set_global("_G", LuaVal::Table(self.globals));
        for name in [
            "type", "tostring", "tonumber", "pairs", "ipairs", "next", "select",
            "pcall", "xpcall", "error", "assert", "rawget", "rawset", "rawequal",
            "setmetatable", "getmetatable", "unpack", "print", "GetID",
        ] {
            self.set_global(name, LuaVal::NativeFn(format!("std.{name}")));
        }

        // `table` library
        let tbl_id = self.alloc_table();
        for f in ["insert", "remove", "concat", "sort", "unpack", "move"] {
            self.set_table_field(tbl_id, f, LuaVal::NativeFn(format!("table.{f}")));
        }
        self.set_global("table", LuaVal::Table(tbl_id));

        // `math` library
        let math_id = self.alloc_table();
        for f in ["abs", "floor", "ceil", "min", "max", "sqrt", "fmod", "random"] {
            self.set_table_field(math_id, f, LuaVal::NativeFn(format!("math.{f}")));
        }
        self.set_table_field(math_id, "huge", LuaVal::Float(f64::INFINITY));
        self.set_table_field(math_id, "pi", LuaVal::Float(std::f64::consts::PI));
        self.set_global("math", LuaVal::Table(math_id));

        // `string` library
        let str_id = self.alloc_table();
        for f in ["len", "sub", "lower", "upper", "find", "format", "byte", "char"] {
            self.set_table_field(str_id, f, LuaVal::NativeFn(format!("string.{f}")));
        }
        self.set_global("string", LuaVal::Table(str_id));

        // `bit32` & `bit` libraries (legacy YGOPro compatibility!)
        let bit_id = self.alloc_table();
        for f in ["band", "bor", "bxor", "bnot", "lshift", "rshift"] {
            self.set_table_field(bit_id, f, LuaVal::NativeFn(format!("bit.{f}")));
        }
        self.set_global("bit32", LuaVal::Table(bit_id));
        self.set_global("bit", LuaVal::Table(bit_id));
    }

    /// Execute a Lua chunk directly (with optional active `DuelEngine`).
    pub fn exec(&mut self, src: &str, engine: Option<&mut DuelEngine>) -> Result<Vec<LuaVal>, LuaError> {
        let stmts = parse(src)?;
        let env = self.alloc_env(None);
        let mut eng = engine;
        match self.exec_block(&stmts, env, &[], &mut eng)? {
            ControlFlow::Return(vals) => Ok(vals),
            _ => Ok(Vec::new()),
        }
    }

    /// Load an EDOPro/YGOPro/Omega card script for `card_code` and return its script table (`s` or `c<id>`).
    pub fn load_card_script(
        &mut self,
        card_code: CardCode,
        src: &str,
        engine: &mut DuelEngine,
    ) -> Result<TableId, LuaError> {
        let script_tid = self.alloc_table();
        let prev_tbl = self.current_script_table;
        let prev_code = self.current_script_code;
        let prev_self_table = self.get_global("self_table");
        let prev_self_code = self.get_global("self_code");
        self.current_script_table = Some(script_tid);
        self.current_script_code = card_code;
        self.set_global("self_table", LuaVal::Table(script_tid));
        self.set_global("self_code", LuaVal::Int(card_code as i64));

        // Also bind legacy global `c<code> = script_table` (classic YGOPro scripts!)
        self.set_global(&format!("c{card_code}"), LuaVal::Table(script_tid));
        let res = self.exec(src, Some(engine));

        self.current_script_table = prev_tbl;
        self.current_script_code = prev_code;
        self.set_global("self_table", prev_self_table);
        self.set_global("self_code", prev_self_code);
        res?;
        Ok(script_tid)
    }

    /// Invoke a Lua value (Closure or NativeFn) with arguments and optional `DuelEngine`.
    pub fn call_value(
        &mut self,
        callee: LuaVal,
        args: Vec<LuaVal>,
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<Vec<LuaVal>, LuaError> {
        match callee {
            LuaVal::Closure(fid) => self.call_closure(fid, args, engine),
            LuaVal::NativeFn(name) => self.call_native(&name, args, engine),
            LuaVal::Table(tid) => {
                // Support `__call` metamethod
                let mm = self
                    .tables
                    .get(&tid)
                    .and_then(|t| t.metatable)
                    .and_then(|mt| self.tables.get(&mt))
                    .map(|mt| mt.get(&TableKey::Str("__call".into())))
                    .unwrap_or(LuaVal::Nil);
                if !matches!(mm, LuaVal::Nil) {
                    let mut full_args = Vec::with_capacity(args.len() + 1);
                    full_args.push(LuaVal::Table(tid));
                    full_args.extend(args);
                    self.call_value(mm, full_args, engine)
                } else {
                    Err(LuaError::Runtime(format!("attempt to call table:{tid}")))
                }
            }
            other => Err(LuaError::Runtime(format!(
                "attempt to call a {} value ({})",
                other.type_name(),
                other.as_str()
            ))),
        }
    }

    pub fn call_closure(
        &mut self,
        fid: FuncId,
        args: Vec<LuaVal>,
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<Vec<LuaVal>, LuaError> {
        let closure = self
            .closures
            .get(&fid)
            .cloned()
            .ok_or_else(|| LuaError::Runtime(format!("closure {fid} not found")))?;
        let env = self.alloc_env(Some(closure.captured_env));
        for (i, param) in closure.params.iter().enumerate() {
            let v = args.get(i).cloned().unwrap_or(LuaVal::Nil);
            if let Some(e) = self.envs.get_mut(&env) {
                e.vars.insert(param.clone(), v);
            }
        }
        let varargs = if closure.vararg && args.len() > closure.params.len() {
            args[closure.params.len()..].to_vec()
        } else {
            Vec::new()
        };
        match self.exec_block(&closure.body, env, &varargs, engine)? {
            ControlFlow::Return(vals) => Ok(vals),
            _ => Ok(Vec::new()),
        }
    }

    fn exec_block(
        &mut self,
        stmts: &[Stmt],
        env: EnvId,
        varargs: &[LuaVal],
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<ControlFlow, LuaError> {
        for stmt in stmts {
            let cf = self.exec_stmt(stmt, env, varargs, engine)?;
            if !matches!(cf, ControlFlow::None) {
                return Ok(cf);
            }
        }
        Ok(ControlFlow::None)
    }

    fn exec_stmt(
        &mut self,
        stmt: &Stmt,
        env: EnvId,
        varargs: &[LuaVal],
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<ControlFlow, LuaError> {
        match stmt {
            Stmt::Local { names, values } => {
                let evaled = self.eval_expr_list(values, env, varargs, engine)?;
                for (i, name) in names.iter().enumerate() {
                    let v = evaled.get(i).cloned().unwrap_or(LuaVal::Nil);
                    if let Some(e) = self.envs.get_mut(&env) {
                        e.vars.insert(name.clone(), v);
                    }
                }
            }
            Stmt::Assign { targets, values } => {
                let evaled = self.eval_expr_list(values, env, varargs, engine)?;
                for (i, target) in targets.iter().enumerate() {
                    let v = evaled.get(i).cloned().unwrap_or(LuaVal::Nil);
                    self.assign_target(target, v, env, varargs, engine)?;
                }
            }
            Stmt::ExprStmt(expr) => {
                let _ = self.eval_expr_multi(expr, env, varargs, engine)?;
            }
            Stmt::Do(body) => {
                let child = self.alloc_env(Some(env));
                return self.exec_block(body, child, varargs, engine);
            }
            Stmt::If { branches, else_body } => {
                for (cond, body) in branches {
                    if self.eval_expr(cond, env, varargs, engine)?.is_truthy() {
                        let child = self.alloc_env(Some(env));
                        return self.exec_block(body, child, varargs, engine);
                    }
                }
                if let Some(eb) = else_body {
                    let child = self.alloc_env(Some(env));
                    return self.exec_block(eb, child, varargs, engine);
                }
            }
            Stmt::While { cond, body } => {
                let mut guard = 0usize;
                while self.eval_expr(cond, env, varargs, engine)?.is_truthy() {
                    guard += 1;
                    if guard > 100_000 {
                        return Err(LuaError::Runtime("while loop limit exceeded".into()));
                    }
                    let child = self.alloc_env(Some(env));
                    match self.exec_block(body, child, varargs, engine)? {
                        ControlFlow::Break => break,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::None => {}
                    }
                }
            }
            Stmt::Repeat { body, cond } => {
                let mut guard = 0usize;
                loop {
                    guard += 1;
                    if guard > 100_000 {
                        return Err(LuaError::Runtime("repeat loop limit exceeded".into()));
                    }
                    let child = self.alloc_env(Some(env));
                    match self.exec_block(body, child, varargs, engine)? {
                        ControlFlow::Break => break,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::None => {}
                    }
                    if self.eval_expr(cond, child, varargs, engine)?.is_truthy() {
                        break;
                    }
                }
            }
            Stmt::ForNum { var, start, stop, step, body } => {
                let s = self.eval_expr(start, env, varargs, engine)?.as_int();
                let e = self.eval_expr(stop, env, varargs, engine)?.as_int();
                let st = match step {
                    Some(se) => self.eval_expr(se, env, varargs, engine)?.as_int(),
                    None => 1,
                };
                if st == 0 {
                    return Err(LuaError::Runtime("'for' step is zero".into()));
                }
                let mut cur = s;
                while (st > 0 && cur <= e) || (st < 0 && cur >= e) {
                    let child = self.alloc_env(Some(env));
                    if let Some(ce) = self.envs.get_mut(&child) {
                        ce.vars.insert(var.clone(), LuaVal::Int(cur));
                    }
                    match self.exec_block(body, child, varargs, engine)? {
                        ControlFlow::Break => break,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::None => {}
                    }
                    cur = cur.saturating_add(st);
                }
            }
            Stmt::ForIn { vars, iters, body } => {
                let evaled = self.eval_expr_list(iters, env, varargs, engine)?;
                let iter_fn = evaled.first().cloned().unwrap_or(LuaVal::Nil);
                let state = evaled.get(1).cloned().unwrap_or(LuaVal::Nil);
                let mut control = evaled.get(2).cloned().unwrap_or(LuaVal::Nil);
                let mut guard = 0usize;
                loop {
                    guard += 1;
                    if guard > 100_000 {
                        return Err(LuaError::Runtime("for-in loop limit exceeded".into()));
                    }
                    let res = self.call_value(iter_fn.clone(), vec![state.clone(), control.clone()], engine)?;
                    let first = res.first().cloned().unwrap_or(LuaVal::Nil);
                    if matches!(first, LuaVal::Nil) {
                        break;
                    }
                    control = first;
                    let child = self.alloc_env(Some(env));
                    for (i, vname) in vars.iter().enumerate() {
                        let val = res.get(i).cloned().unwrap_or(LuaVal::Nil);
                        if let Some(ce) = self.envs.get_mut(&child) {
                            ce.vars.insert(vname.clone(), val);
                        }
                    }
                    match self.exec_block(body, child, varargs, engine)? {
                        ControlFlow::Break => break,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::None => {}
                    }
                }
            }
            Stmt::Return(exprs) => {
                let vals = self.eval_expr_list(exprs, env, varargs, engine)?;
                return Ok(ControlFlow::Return(vals));
            }
            Stmt::Break => return Ok(ControlFlow::Break),
        }
        Ok(ControlFlow::None)
    }

    fn assign_target(
        &mut self,
        target: &Expr,
        val: LuaVal,
        env: EnvId,
        varargs: &[LuaVal],
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<(), LuaError> {
        match target {
            Expr::Ident(name) => {
                // Walk lexical scope chain; if found in a local env update it, else set global!
                let mut cur = Some(env);
                while let Some(eid) = cur {
                    if let Some(e) = self.envs.get_mut(&eid) {
                        if e.vars.contains_key(name) {
                            e.vars.insert(name.clone(), val);
                            return Ok(());
                        }
                        cur = e.parent;
                    } else {
                        break;
                    }
                }
                self.set_global(name, val);
                Ok(())
            }
            Expr::Field { target, name } => {
                let obj = self.eval_expr(target, env, varargs, engine)?;
                match obj {
                    LuaVal::Table(tid) => {
                        self.set_table_field(tid, name, val);
                        Ok(())
                    }
                    LuaVal::Card(cid) => {
                        let mt = self.card_metatable(cid);
                        self.set_table_field(mt, name, val);
                        Ok(())
                    }
                    _ => Err(LuaError::Runtime(format!("cannot set field '{name}' on {}", obj.type_name()))),
                }
            }
            Expr::Index { target, key } => {
                let obj = self.eval_expr(target, env, varargs, engine)?;
                let k = self.eval_expr(key, env, varargs, engine)?;
                if let LuaVal::Table(tid) = obj {
                    if let Some(tk) = k.to_key() {
                        if let Some(t) = self.tables.get_mut(&tid) {
                            t.set(tk, val);
                        }
                    }
                    Ok(())
                } else {
                    Err(LuaError::Runtime(format!("cannot index {}", obj.type_name())))
                }
            }
            _ => Err(LuaError::Runtime("invalid assignment target".into())),
        }
    }

    fn lookup_var(&self, name: &str, env: EnvId) -> LuaVal {
        let mut cur = Some(env);
        while let Some(eid) = cur {
            if let Some(e) = self.envs.get(&eid) {
                if let Some(v) = e.vars.get(name) {
                    return v.clone();
                }
                cur = e.parent;
            } else {
                break;
            }
        }
        self.get_global(name)
    }

    fn eval_expr_list(
        &mut self,
        exprs: &[Expr],
        env: EnvId,
        varargs: &[LuaVal],
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<Vec<LuaVal>, LuaError> {
        let mut out = Vec::new();
        for (idx, e) in exprs.iter().enumerate() {
            if idx + 1 == exprs.len() {
                // Last expression expands all multiple return values (official Lua semantics!)
                out.extend(self.eval_expr_multi(e, env, varargs, engine)?);
            } else {
                out.push(self.eval_expr(e, env, varargs, engine)?);
            }
        }
        Ok(out)
    }

    fn eval_expr(
        &mut self,
        expr: &Expr,
        env: EnvId,
        varargs: &[LuaVal],
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<LuaVal, LuaError> {
        Ok(self
            .eval_expr_multi(expr, env, varargs, engine)?
            .into_iter()
            .next()
            .unwrap_or(LuaVal::Nil))
    }

    fn eval_expr_multi(
        &mut self,
        expr: &Expr,
        env: EnvId,
        varargs: &[LuaVal],
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<Vec<LuaVal>, LuaError> {
        let single = match expr {
            Expr::Nil => LuaVal::Nil,
            Expr::Bool(b) => LuaVal::Bool(*b),
            Expr::Int(i) => LuaVal::Int(*i),
            Expr::Float(f) => LuaVal::Float(*f),
            Expr::Str(s) => LuaVal::Str(s.clone()),
            Expr::VarArg => return Ok(varargs.to_vec()),
            Expr::Ident(name) => self.lookup_var(name, env),
            Expr::Func { params, vararg, body } => {
                let fid = self.next_func_id;
                self.next_func_id += 1;
                self.closures.insert(
                    fid,
                    LuaClosure {
                        params: params.clone(),
                        vararg: *vararg,
                        body: body.clone(),
                        captured_env: env,
                    },
                );
                LuaVal::Closure(fid)
            }
            Expr::Table(fields) => {
                let tid = self.alloc_table();
                let mut arr_idx = 1i64;
                for (fidx, field) in fields.iter().enumerate() {
                    match field {
                        TableField::Indexed(k, v) => {
                            let kv = self.eval_expr(k, env, varargs, engine)?;
                            let vv = self.eval_expr(v, env, varargs, engine)?;
                            if let Some(tk) = kv.to_key() {
                                if let Some(t) = self.tables.get_mut(&tid) {
                                    t.set(tk, vv);
                                }
                            }
                        }
                        TableField::Named(name, v) => {
                            let vv = self.eval_expr(v, env, varargs, engine)?;
                            self.set_table_field(tid, name, vv);
                        }
                        TableField::Array(v) => {
                            if fidx + 1 == fields.len() {
                                let vals = self.eval_expr_multi(v, env, varargs, engine)?;
                                for vv in vals {
                                    if let Some(t) = self.tables.get_mut(&tid) {
                                        t.set(TableKey::Int(arr_idx), vv);
                                    }
                                    arr_idx += 1;
                                }
                            } else {
                                let vv = self.eval_expr(v, env, varargs, engine)?;
                                if let Some(t) = self.tables.get_mut(&tid) {
                                    t.set(TableKey::Int(arr_idx), vv);
                                }
                                arr_idx += 1;
                            }
                        }
                    }
                }
                LuaVal::Table(tid)
            }
            Expr::Unary { op, expr } => {
                let v = self.eval_expr(expr, env, varargs, engine)?;
                match op {
                    UnOp::Not => LuaVal::Bool(!v.is_truthy()),
                    UnOp::Neg => match v {
                        LuaVal::Int(i) => LuaVal::Int(-i),
                        LuaVal::Float(f) => LuaVal::Float(-f),
                        _ => LuaVal::Int(-v.as_int()),
                    },
                    UnOp::BitNot => LuaVal::Int(!v.as_int()),
                    UnOp::Len => match v {
                        LuaVal::Str(s) => LuaVal::Int(s.len() as i64),
                        LuaVal::Table(tid) => {
                            let l = self.tables.get(&tid).map(|t| t.seq_len()).unwrap_or(0);
                            LuaVal::Int(l as i64)
                        }
                        LuaVal::Group(gid) => {
                            let l = self.groups.get(&gid).map(|g| g.cards.len()).unwrap_or(0);
                            LuaVal::Int(l as i64)
                        }
                        _ => LuaVal::Int(0),
                    },
                }
            }
            Expr::Binary { op, left, right } => {
                // Short-circuit `and` & `or` (official Lua returns operand value, not bool!)
                if matches!(op, BinOp::And) {
                    let lv = self.eval_expr(left, env, varargs, engine)?;
                    return Ok(vec![if !lv.is_truthy() {
                        lv
                    } else {
                        self.eval_expr(right, env, varargs, engine)?
                    }]);
                }
                if matches!(op, BinOp::Or) {
                    let lv = self.eval_expr(left, env, varargs, engine)?;
                    return Ok(vec![if lv.is_truthy() {
                        lv
                    } else {
                        self.eval_expr(right, env, varargs, engine)?
                    }]);
                }
                let lv = self.eval_expr(left, env, varargs, engine)?;
                let rv = self.eval_expr(right, env, varargs, engine)?;
                eval_binop(*op, lv, rv)
            }
            Expr::Field { target, name } => {
                let obj = self.eval_expr(target, env, varargs, engine)?;
                self.resolve_field(&obj, name)
            }
            Expr::Index { target, key } => {
                let obj = self.eval_expr(target, env, varargs, engine)?;
                let k = self.eval_expr(key, env, varargs, engine)?;
                match obj {
                    LuaVal::Table(tid) => k
                        .to_key()
                        .map(|tk| self.get_table_val(tid, &tk))
                        .unwrap_or(LuaVal::Nil),
                    _ => LuaVal::Nil,
                }
            }
            Expr::Call { callee, args } => {
                let f = self.eval_expr(callee, env, varargs, engine)?;
                let eval_args = self.eval_expr_list(args, env, varargs, engine)?;
                return self.call_value(f, eval_args, engine).map_err(|e| {
                    LuaError::Runtime(format!("{e} while calling {:?}", callee))
                });
            }
            Expr::MethodCall { target, method, args } => {
                let obj = self.eval_expr(target, env, varargs, engine)?;
                let f = self.resolve_field(&obj, method);
                let mut eval_args = Vec::with_capacity(args.len() + 1);
                eval_args.push(obj);
                eval_args.extend(self.eval_expr_list(args, env, varargs, engine)?);
                return self.call_value(f, eval_args, engine).map_err(|e| {
                    LuaError::Runtime(format!("{e} while calling method :{method}"))
                });
            }
        };
        Ok(vec![single])
    }

    pub fn card_metatable(&mut self, cid: CardId) -> TableId {
        if let Some(&tid) = self.card_metatables.get(&cid) {
            return tid;
        }
        let tid = self.alloc_table();
        self.card_metatables.insert(cid, tid);
        tid
    }

    fn resolve_field(&self, obj: &LuaVal, name: &str) -> LuaVal {
        match obj {
            LuaVal::Table(tid) => self.get_table_val(*tid, &TableKey::Str(name.to_string())),
            LuaVal::Card(cid) => {
                if name == "__index" {
                    if let Some(&mt) = self.card_metatables.get(cid) {
                        return LuaVal::Table(mt);
                    }
                }
                if let Some(&mt) = self.card_metatables.get(cid) {
                    let v = self.get_table_val(mt, &TableKey::Str(name.to_string()));
                    if !matches!(v, LuaVal::Nil) {
                        return v;
                    }
                }
                self.lookup_type_method("Card", name)
            }
            LuaVal::Effect(_) => self.lookup_type_method("Effect", name),
            LuaVal::Group(_) => self.lookup_type_method("Group", name),
            LuaVal::Str(_) => self.lookup_type_method("string", name),
            _ => LuaVal::Nil,
        }
    }

    fn lookup_type_method(&self, ns: &str, name: &str) -> LuaVal {
        if let LuaVal::Table(tid) = self.get_global(ns) {
            let v = self.get_table_val(tid, &TableKey::Str(name.to_string()));
            if !matches!(v, LuaVal::Nil) {
                return v;
            }
        }
        LuaVal::NativeFn(format!("{ns}.{name}"))
    }

    fn call_native(
        &mut self,
        name: &str,
        args: Vec<LuaVal>,
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<Vec<LuaVal>, LuaError> {
        if let Some(rest) = name.strip_prefix("std.") {
            return self.call_std(rest, args, engine);
        }
        if let Some(rest) = name.strip_prefix("table.") {
            return self.call_table_lib(rest, args);
        }
        if let Some(rest) = name.strip_prefix("math.") {
            return Ok(vec![self.call_math_lib(rest, args)]);
        }
        if let Some(rest) = name.strip_prefix("string.") {
            return Ok(vec![self.call_string_lib(rest, args)]);
        }
        if let Some(rest) = name.strip_prefix("bit.") {
            return Ok(vec![self.call_bit_lib(rest, args)]);
        }
        call_ocg_native(self, name, args, engine)
    }

    fn call_std(
        &mut self,
        name: &str,
        args: Vec<LuaVal>,
        engine: &mut Option<&mut DuelEngine>,
    ) -> Result<Vec<LuaVal>, LuaError> {
        match name {
            "GetID" => {
                let tid = self.current_script_table.unwrap_or_else(|| self.alloc_table());
                Ok(vec![LuaVal::Table(tid), LuaVal::Int(self.current_script_code as i64)])
            }
            "type" => {
                let t = args.first().map(|v| v.type_name()).unwrap_or("nil");
                Ok(vec![LuaVal::Str(t.into())])
            }
            "tostring" => {
                let s = args.first().map(|v| v.as_str()).unwrap_or_else(|| "nil".into());
                Ok(vec![LuaVal::Str(s)])
            }
            "tonumber" => {
                let Some(v) = args.first() else {
                    return Ok(vec![LuaVal::Nil]);
                };
                match v {
                    LuaVal::Int(i) => Ok(vec![LuaVal::Int(*i)]),
                    LuaVal::Float(f) => Ok(vec![LuaVal::Float(*f)]),
                    LuaVal::Str(s) => {
                        let base = args.get(1).map(|b| b.as_int() as u32).unwrap_or(10);
                        if let Ok(i) = i64::from_str_radix(s.trim(), base) {
                            Ok(vec![LuaVal::Int(i)])
                        } else if let Ok(f) = s.trim().parse::<f64>() {
                            Ok(vec![LuaVal::Float(f)])
                        } else {
                            Ok(vec![LuaVal::Nil])
                        }
                    }
                    _ => Ok(vec![LuaVal::Nil]),
                }
            }
            "select" => {
                let Some(first) = args.first() else {
                    return Ok(vec![LuaVal::Nil]);
                };
                if matches!(first, LuaVal::Str(s) if s == "#") {
                    return Ok(vec![LuaVal::Int((args.len().saturating_sub(1)) as i64)]);
                }
                let idx = first.as_int();
                if idx >= 1 && (idx as usize) < args.len() {
                    Ok(args[(idx as usize)..].to_vec())
                } else {
                    Ok(vec![])
                }
            }
            "pairs" => {
                let tbl = args.first().cloned().unwrap_or(LuaVal::Nil);
                Ok(vec![LuaVal::NativeFn("std.next".into()), tbl, LuaVal::Nil])
            }
            "ipairs" => {
                let tbl = args.first().cloned().unwrap_or(LuaVal::Nil);
                Ok(vec![LuaVal::NativeFn("std.__ipairs_next".into()), tbl, LuaVal::Int(0)])
            }
            "__ipairs_next" => {
                let idx = args.get(1).map(|v| v.as_int()).unwrap_or(0) + 1;
                if let Some(LuaVal::Table(tid)) = args.first() {
                    let v = self.get_table_val(*tid, &TableKey::Int(idx));
                    if !matches!(v, LuaVal::Nil) {
                        return Ok(vec![LuaVal::Int(idx), v]);
                    }
                }
                Ok(vec![LuaVal::Nil])
            }
            "next" => {
                let Some(LuaVal::Table(tid)) = args.first() else {
                    return Ok(vec![LuaVal::Nil]);
                };
                let cur_key = args.get(1).and_then(|v| v.to_key());
                if let Some(t) = self.tables.get(tid) {
                    let mut iter = t.entries.iter();
                    if let Some(ck) = cur_key {
                        while let Some((k, _)) = iter.next() {
                            if k == &ck {
                                break;
                            }
                        }
                    }
                    if let Some((nk, nv)) = iter.next() {
                        return Ok(vec![nk.to_val(), nv.clone()]);
                    }
                }
                Ok(vec![LuaVal::Nil])
            }
            "pcall" => {
                let Some(f) = args.first().cloned() else {
                    return Ok(vec![LuaVal::Bool(false), LuaVal::Str("missing function".into())]);
                };
                match self.call_value(f, args[1..].to_vec(), engine) {
                    Ok(mut res) => {
                        let mut out = vec![LuaVal::Bool(true)];
                        out.append(&mut res);
                        Ok(out)
                    }
                    Err(e) => Ok(vec![LuaVal::Bool(false), LuaVal::Str(e.to_string())]),
                }
            }
            "assert" => {
                let cond = args.first().map(|v| v.is_truthy()).unwrap_or(false);
                if cond {
                    Ok(args)
                } else {
                    let msg = args.get(1).map(|v| v.as_str()).unwrap_or_else(|| "assertion failed".into());
                    Err(LuaError::Runtime(msg))
                }
            }
            "error" => {
                let msg = args.first().map(|v| v.as_str()).unwrap_or_else(|| "error".into());
                Err(LuaError::Runtime(msg))
            }
            "rawget" => {
                if let (Some(LuaVal::Table(tid)), Some(k)) = (args.first(), args.get(1).and_then(|v| v.to_key())) {
                    Ok(vec![self.tables.get(tid).map(|t| t.get(&k)).unwrap_or(LuaVal::Nil)])
                } else {
                    Ok(vec![LuaVal::Nil])
                }
            }
            "rawset" => {
                if let (Some(LuaVal::Table(tid)), Some(k)) = (args.first(), args.get(1).and_then(|v| v.to_key())) {
                    let v = args.get(2).cloned().unwrap_or(LuaVal::Nil);
                    if let Some(t) = self.tables.get_mut(tid) {
                        t.set(k, v);
                    }
                    Ok(vec![LuaVal::Table(*tid)])
                } else {
                    Ok(vec![LuaVal::Nil])
                }
            }
            "rawequal" => {
                let a = args.first().unwrap_or(&LuaVal::Nil);
                let b = args.get(1).unwrap_or(&LuaVal::Nil);
                Ok(vec![LuaVal::Bool(a == b)])
            }
            "setmetatable" => {
                if let Some(LuaVal::Table(tid)) = args.first() {
                    let mt = match args.get(1) {
                        Some(LuaVal::Table(m)) => Some(*m),
                        _ => None,
                    };
                    if let Some(t) = self.tables.get_mut(tid) {
                        t.metatable = mt;
                    }
                    Ok(vec![LuaVal::Table(*tid)])
                } else {
                    Ok(vec![LuaVal::Nil])
                }
            }
            "getmetatable" => match args.first() {
                Some(LuaVal::Table(tid)) => {
                    let mt = self.tables.get(tid).and_then(|t| t.metatable);
                    Ok(vec![mt.map(LuaVal::Table).unwrap_or(LuaVal::Nil)])
                }
                Some(LuaVal::Card(cid)) => {
                    let mt = self.card_metatable(*cid);
                    Ok(vec![LuaVal::Table(mt)])
                }
                _ => Ok(vec![LuaVal::Nil]),
            },
            "unpack" => self.call_table_lib("unpack", args),
            "print" => {
                let line: Vec<String> = args.iter().map(|v| v.as_str()).collect();
                self.output_log.push(line.join("\t"));
                Ok(vec![])
            }
            _ => Ok(vec![LuaVal::Nil]),
        }
    }

    fn call_table_lib(&mut self, name: &str, args: Vec<LuaVal>) -> Result<Vec<LuaVal>, LuaError> {
        let Some(LuaVal::Table(tid)) = args.first() else {
            return Ok(vec![LuaVal::Nil]);
        };
        let tid = *tid;
        match name {
            "insert" => {
                let len = self.tables.get(&tid).map(|t| t.seq_len()).unwrap_or(0) as i64;
                if args.len() == 2 {
                    if let Some(t) = self.tables.get_mut(&tid) {
                        t.set(TableKey::Int(len + 1), args[1].clone());
                    }
                } else if args.len() >= 3 {
                    let pos = args[1].as_int().clamp(1, len + 1);
                    if let Some(t) = self.tables.get_mut(&tid) {
                        for i in (pos..=len).rev() {
                            let v = t.get(&TableKey::Int(i));
                            t.set(TableKey::Int(i + 1), v);
                        }
                        t.set(TableKey::Int(pos), args[2].clone());
                    }
                }
                Ok(vec![])
            }
            "remove" => {
                let len = self.tables.get(&tid).map(|t| t.seq_len()).unwrap_or(0) as i64;
                if len == 0 {
                    return Ok(vec![LuaVal::Nil]);
                }
                let pos = args.get(1).map(|v| v.as_int()).unwrap_or(len).clamp(1, len);
                let removed = if let Some(t) = self.tables.get_mut(&tid) {
                    let r = t.get(&TableKey::Int(pos));
                    for i in pos..len {
                        let next = t.get(&TableKey::Int(i + 1));
                        t.set(TableKey::Int(i), next);
                    }
                    t.set(TableKey::Int(len), LuaVal::Nil);
                    r
                } else {
                    LuaVal::Nil
                };
                Ok(vec![removed])
            }
            "concat" => {
                let sep = args.get(1).map(|v| v.as_str()).unwrap_or_default();
                let len = self.tables.get(&tid).map(|t| t.seq_len()).unwrap_or(0) as i64;
                let mut parts = Vec::new();
                for i in 1..=len {
                    parts.push(self.get_table_val(tid, &TableKey::Int(i)).as_str());
                }
                Ok(vec![LuaVal::Str(parts.join(&sep))])
            }
            "unpack" => {
                let len = self.tables.get(&tid).map(|t| t.seq_len()).unwrap_or(0) as i64;
                let i = args.get(1).map(|v| v.as_int()).unwrap_or(1);
                let j = args.get(2).map(|v| v.as_int()).unwrap_or(len);
                let mut out = Vec::new();
                for k in i..=j {
                    out.push(self.get_table_val(tid, &TableKey::Int(k)));
                }
                Ok(out)
            }
            _ => Ok(vec![LuaVal::Nil]),
        }
    }

    fn call_math_lib(&self, name: &str, args: Vec<LuaVal>) -> LuaVal {
        match name {
            "abs" => match args.first() {
                Some(LuaVal::Int(i)) => LuaVal::Int(i.abs()),
                Some(v) => LuaVal::Float(v.as_float().abs()),
                None => LuaVal::Int(0),
            },
            "floor" => LuaVal::Int(args.first().map(|v| v.as_float().floor() as i64).unwrap_or(0)),
            "ceil" => LuaVal::Int(args.first().map(|v| v.as_float().ceil() as i64).unwrap_or(0)),
            "min" => {
                let m = args.iter().map(|v| v.as_int()).min().unwrap_or(0);
                LuaVal::Int(m)
            }
            "max" => {
                let m = args.iter().map(|v| v.as_int()).max().unwrap_or(0);
                LuaVal::Int(m)
            }
            "sqrt" => LuaVal::Float(args.first().map(|v| v.as_float().sqrt()).unwrap_or(0.0)),
            _ => LuaVal::Int(0),
        }
    }

    fn call_string_lib(&self, name: &str, args: Vec<LuaVal>) -> LuaVal {
        let s = args.first().map(|v| v.as_str()).unwrap_or_default();
        match name {
            "len" => LuaVal::Int(s.len() as i64),
            "lower" => LuaVal::Str(s.to_lowercase()),
            "upper" => LuaVal::Str(s.to_uppercase()),
            "sub" => {
                let chars: Vec<char> = s.chars().collect();
                let n = chars.len() as i64;
                let mut i = args.get(1).map(|v| v.as_int()).unwrap_or(1);
                let mut j = args.get(2).map(|v| v.as_int()).unwrap_or(n);
                if i < 0 { i = n + i + 1; }
                if j < 0 { j = n + j + 1; }
                let start = (i.max(1) - 1) as usize;
                let end = j.min(n).max(0) as usize;
                if start >= end || start >= chars.len() {
                    LuaVal::Str(String::new())
                } else {
                    LuaVal::Str(chars[start..end].iter().collect())
                }
            }
            "find" => {
                let pat = args.get(1).map(|v| v.as_str()).unwrap_or_default();
                if let Some(pos) = s.find(&pat) {
                    LuaVal::Int((pos + 1) as i64)
                } else {
                    LuaVal::Nil
                }
            }
            "format" => LuaVal::Str(s),
            _ => LuaVal::Nil,
        }
    }

    fn call_bit_lib(&self, name: &str, args: Vec<LuaVal>) -> LuaVal {
        let a = args.first().map(|v| v.as_int()).unwrap_or(0);
        let b = args.get(1).map(|v| v.as_int()).unwrap_or(0);
        match name {
            "band" => LuaVal::Int(args.iter().fold(!0i64, |acc, v| acc & v.as_int())),
            "bor" => LuaVal::Int(args.iter().fold(0i64, |acc, v| acc | v.as_int())),
            "bxor" => LuaVal::Int(a ^ b),
            "bnot" => LuaVal::Int(!a),
            "lshift" => LuaVal::Int(a.wrapping_shl(b as u32)),
            "rshift" => LuaVal::Int(((a as u64).wrapping_shr(b as u32)) as i64),
            _ => LuaVal::Int(0),
        }
    }
}

fn eval_binop(op: BinOp, lv: LuaVal, rv: LuaVal) -> LuaVal {
    match op {
        BinOp::Concat => LuaVal::Str(format!("{}{}", lv.as_str(), rv.as_str())),
        BinOp::Eq => LuaVal::Bool(lua_eq(&lv, &rv)),
        BinOp::Ne => LuaVal::Bool(!lua_eq(&lv, &rv)),
        BinOp::Lt => LuaVal::Bool(lv.as_float() < rv.as_float()),
        BinOp::Le => LuaVal::Bool(lv.as_float() <= rv.as_float()),
        BinOp::Gt => LuaVal::Bool(lv.as_float() > rv.as_float()),
        BinOp::Ge => LuaVal::Bool(lv.as_float() >= rv.as_float()),
        BinOp::BitAnd => LuaVal::Int(lv.as_int() & rv.as_int()),
        BinOp::BitOr => LuaVal::Int(lv.as_int() | rv.as_int()),
        BinOp::BitXor => LuaVal::Int(lv.as_int() ^ rv.as_int()),
        BinOp::Shl => LuaVal::Int(lv.as_int().wrapping_shl(rv.as_int() as u32)),
        BinOp::Shr => LuaVal::Int(((lv.as_int() as u64).wrapping_shr(rv.as_int() as u32)) as i64),
        BinOp::Add => match (lv, rv) {
            (LuaVal::Int(a), LuaVal::Int(b)) => LuaVal::Int(a.wrapping_add(b)),
            (a, b) => LuaVal::Float(a.as_float() + b.as_float()),
        },
        BinOp::Sub => match (lv, rv) {
            (LuaVal::Int(a), LuaVal::Int(b)) => LuaVal::Int(a.wrapping_sub(b)),
            (a, b) => LuaVal::Float(a.as_float() - b.as_float()),
        },
        BinOp::Mul => match (lv, rv) {
            (LuaVal::Int(a), LuaVal::Int(b)) => LuaVal::Int(a.wrapping_mul(b)),
            (a, b) => LuaVal::Float(a.as_float() * b.as_float()),
        },
        BinOp::Div => LuaVal::Float(lv.as_float() / rv.as_float()),
        BinOp::FloorDiv => {
            let b = rv.as_int();
            if b == 0 { LuaVal::Int(0) } else { LuaVal::Int(lv.as_int().div_euclid(b)) }
        }
        BinOp::Mod => {
            let b = rv.as_int();
            if b == 0 { LuaVal::Int(0) } else { LuaVal::Int(lv.as_int().rem_euclid(b)) }
        }
        BinOp::Pow => LuaVal::Float(lv.as_float().powf(rv.as_float())),
        BinOp::And | BinOp::Or => unreachable!(),
    }
}

fn lua_eq(a: &LuaVal, b: &LuaVal) -> bool {
    match (a, b) {
        (LuaVal::Int(x), LuaVal::Float(y)) => (*x as f64) == *y,
        (LuaVal::Float(x), LuaVal::Int(y)) => *x == (*y as f64),
        _ => a == b,
    }
}

/// Helper to build standard ocgcore callback arguments `(e, tp, eg, ep, ev, re, r, rp, [chk])`.
pub fn build_callback_args(
    vm: &mut LuaVm,
    effect_id: EffectId,
    tp: PlayerId,
    event: Option<&DuelEvent>,
    chk: Option<i32>,
) -> Vec<LuaVal> {
    let eg_cards = event.map(|e| e.cards.clone()).unwrap_or_default();
    let eg_id = vm.alloc_group(eg_cards);
    let ep = event.map(|e| e.player as i64).unwrap_or(tp as i64);
    let ev = event.map(|e| e.value).unwrap_or(0);
    let re = event
        .and_then(|e| e.reason_effect)
        .map(LuaVal::Effect)
        .unwrap_or(LuaVal::Nil);
    let r = event.map(|e| e.reason.bits() as i64).unwrap_or(0);
    let rp = event.map(|e| e.reason_player as i64).unwrap_or(tp as i64);

    let mut args = vec![
        LuaVal::Effect(effect_id),
        LuaVal::Int(tp as i64),
        LuaVal::Group(eg_id),
        LuaVal::Int(ep),
        LuaVal::Int(ev),
        re,
        LuaVal::Int(r),
        LuaVal::Int(rp),
    ];
    if let Some(c) = chk {
        args.push(LuaVal::Int(c as i64));
    }
    args
}
