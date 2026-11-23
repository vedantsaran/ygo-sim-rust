use std::collections::BTreeMap;
use neon_ygo_core::{CardId, EffectId};
use crate::ast::Stmt;

pub type TableId = u32;
pub type FuncId = u32;
pub type GroupId = u32;
pub type EnvId = u32;

/// Pure-Rust Lua 5.4 runtime value including first-class `Card`, `Effect`, and `Group` userdata!
#[derive(Debug, Clone, PartialEq)]
pub enum LuaVal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Table(TableId),
    Closure(FuncId),
    NativeFn(String),
    Card(CardId),
    Effect(EffectId),
    Group(GroupId),
}

impl LuaVal {
    pub fn is_truthy(&self) -> bool {
        !matches!(self, LuaVal::Nil | LuaVal::Bool(false))
    }

    pub fn as_int(&self) -> i64 {
        match self {
            LuaVal::Int(i) => *i,
            LuaVal::Float(f) => *f as i64,
            LuaVal::Bool(b) => if *b { 1 } else { 0 },
            LuaVal::Str(s) => s.trim().parse::<i64>().unwrap_or(0),
            _ => 0,
        }
    }

    pub fn as_u32(&self) -> u32 {
        self.as_int() as u32
    }

    pub fn as_u64(&self) -> u64 {
        self.as_int() as u64
    }

    pub fn as_float(&self) -> f64 {
        match self {
            LuaVal::Float(f) => *f,
            LuaVal::Int(i) => *i as f64,
            LuaVal::Str(s) => s.trim().parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            LuaVal::Nil => "nil".into(),
            LuaVal::Bool(b) => b.to_string(),
            LuaVal::Int(i) => i.to_string(),
            LuaVal::Float(f) => f.to_string(),
            LuaVal::Str(s) => s.clone(),
            LuaVal::Table(id) => format!("table:{id}"),
            LuaVal::Closure(id) => format!("function:{id}"),
            LuaVal::NativeFn(n) => format!("native:{n}"),
            LuaVal::Card(id) => format!("Card({})", id.0),
            LuaVal::Effect(id) => format!("Effect({id})"),
            LuaVal::Group(id) => format!("Group({id})"),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            LuaVal::Nil => "nil",
            LuaVal::Bool(_) => "boolean",
            LuaVal::Int(_) | LuaVal::Float(_) => "number",
            LuaVal::Str(_) => "string",
            LuaVal::Table(_) => "table",
            LuaVal::Closure(_) | LuaVal::NativeFn(_) => "function",
            LuaVal::Card(_) | LuaVal::Effect(_) | LuaVal::Group(_) => "userdata",
        }
    }

    pub fn to_key(&self) -> Option<TableKey> {
        match self {
            LuaVal::Nil => None,
            LuaVal::Bool(b) => Some(TableKey::Bool(*b)),
            LuaVal::Int(i) => Some(TableKey::Int(*i)),
            LuaVal::Float(f) => {
                if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                    Some(TableKey::Int(*f as i64))
                } else {
                    Some(TableKey::Str(f.to_string()))
                }
            }
            LuaVal::Str(s) => Some(TableKey::Str(s.clone())),
            LuaVal::Card(id) => Some(TableKey::Card(id.0)),
            LuaVal::Effect(id) => Some(TableKey::Effect(*id)),
            LuaVal::Group(id) => Some(TableKey::Group(*id)),
            LuaVal::Table(id) => Some(TableKey::Table(*id)),
            LuaVal::Closure(id) => Some(TableKey::Closure(*id)),
            LuaVal::NativeFn(n) => Some(TableKey::Str(format!("@native:{n}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableKey {
    Int(i64),
    Str(String),
    Bool(bool),
    Card(u32),
    Effect(u32),
    Group(u32),
    Table(u32),
    Closure(u32),
}

impl TableKey {
    pub fn to_val(&self) -> LuaVal {
        match self {
            TableKey::Int(i) => LuaVal::Int(*i),
            TableKey::Str(s) => LuaVal::Str(s.clone()),
            TableKey::Bool(b) => LuaVal::Bool(*b),
            TableKey::Card(id) => LuaVal::Card(CardId(*id)),
            TableKey::Effect(id) => LuaVal::Effect(*id),
            TableKey::Group(id) => LuaVal::Group(*id),
            TableKey::Table(id) => LuaVal::Table(*id),
            TableKey::Closure(id) => LuaVal::Closure(*id),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LuaTable {
    pub entries: BTreeMap<TableKey, LuaVal>,
    pub metatable: Option<TableId>,
}

impl LuaTable {
    pub fn get(&self, key: &TableKey) -> LuaVal {
        self.entries.get(key).cloned().unwrap_or(LuaVal::Nil)
    }

    pub fn set(&mut self, key: TableKey, val: LuaVal) {
        if matches!(val, LuaVal::Nil) {
            self.entries.remove(&key);
        } else {
            self.entries.insert(key, val);
        }
    }

    /// Standard Lua sequence length `#t`.
    pub fn seq_len(&self) -> usize {
        let mut n = 0i64;
        while self.entries.contains_key(&TableKey::Int(n + 1)) {
            n += 1;
        }
        n as usize
    }
}

#[derive(Debug, Clone)]
pub struct LuaClosure {
    pub params: Vec<String>,
    pub vararg: bool,
    pub body: Vec<Stmt>,
    pub captured_env: EnvId,
}

#[derive(Debug, Clone, Default)]
pub struct LuaGroup {
    pub cards: Vec<CardId>,
    pub cursor: usize,
}

impl LuaGroup {
    pub fn new(cards: Vec<CardId>) -> Self {
        let mut unique = Vec::new();
        for c in cards {
            if !unique.contains(&c) {
                unique.push(c);
            }
        }
        Self { cards: unique, cursor: 0 }
    }

    pub fn add(&mut self, cid: CardId) {
        if !self.cards.contains(&cid) {
            self.cards.push(cid);
        }
    }

    pub fn remove(&mut self, cid: CardId) {
        self.cards.retain(|&id| id != cid);
    }

    pub fn first(&mut self) -> Option<CardId> {
        self.cursor = 0;
        if self.cards.is_empty() {
            None
        } else {
            self.cursor = 1;
            Some(self.cards[0])
        }
    }

    pub fn next(&mut self) -> Option<CardId> {
        if self.cursor < self.cards.len() {
            let c = self.cards[self.cursor];
            self.cursor += 1;
            Some(c)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LuaEnv {
    pub parent: Option<EnvId>,
    pub vars: BTreeMap<String, LuaVal>,
}
