use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::num::NonZeroU64;

use async_graphql::Value;
use strum_macros::Display;

use super::discriminator::Discriminator;
use super::{EvalContext, ResolverContextLike};
use crate::core::blueprint::{Auth, DynamicValue};
use crate::core::config::group_by::GroupBy;
use crate::core::graphql::{self};
use crate::core::http::HttpFilter;
use crate::core::{grpc, http};

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct IrId(usize);
impl IrId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0 as u64
    }
}

impl Display for IrId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Display)]
pub enum IR {
    Dynamic(DynamicValue<Value>),
    #[strum(to_string = "{0}")]
    IO(IO),
    Cache(Cache),
    // TODO: Path can be implement using Pipe
    Path(Box<IR>, Vec<String>),
    ContextPath(Vec<String>),
    Protect(Auth, Box<IR>),
    Map(Map),
    Pipe(Box<IR>, Box<IR>),
    Discriminate(Discriminator, Box<IR>),
    /// Apollo Federation _entities resolver
    Entity(HashMap<String, IR>),
    /// Apollo Federation _service resolver
    Service(String),
    Deferred {
        id: IrId,
        label: Option<String>,
        ir: Box<IR>,
        path: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Map {
    pub input: Box<IR>,
    // accept key return value instead of
    pub map: HashMap<String, String>,
}

#[derive(Clone, Debug, strum_macros::Display)]
pub enum IO {
    Http {
        req_template: http::RequestTemplate,
        group_by: Option<GroupBy>,
        dl_id: Option<DataLoaderId>,
        http_filter: Option<HttpFilter>,
        is_list: bool,
        dedupe: bool,
        is_dependent: bool,
    },
    GraphQL {
        req_template: graphql::RequestTemplate,
        field_name: String,
        batch: bool,
        dl_id: Option<DataLoaderId>,
        dedupe: bool,
        is_dependent: bool,
    },
    Grpc {
        req_template: grpc::RequestTemplate,
        group_by: Option<GroupBy>,
        dl_id: Option<DataLoaderId>,
        dedupe: bool,
    },
    Js {
        name: String,
    },
}

impl IO {
    // TODO: fix is_dependent for GraphQL and Grpc.
    pub fn is_dependent(&self) -> bool {
        match self {
            IO::Http { is_dependent, .. } => *is_dependent,
            IO::GraphQL { is_dependent, .. } => *is_dependent,
            IO::Grpc { .. } => true,
            IO::Js { .. } => false,
        }
    }

    pub fn dedupe(&self) -> bool {
        match self {
            IO::Http { dedupe, .. } => *dedupe,
            IO::GraphQL { dedupe, .. } => *dedupe,
            IO::Grpc { dedupe, .. } => *dedupe,
            IO::Js { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DataLoaderId(usize);

impl DataLoaderId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }
}

#[derive(PartialEq, Eq, Clone, Hash, Debug)]
pub struct IoId(u64);

impl IoId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

pub trait CacheKey<Ctx> {
    fn cache_key(&self, ctx: &Ctx) -> Option<IoId>;
}

#[derive(Clone, Debug)]
pub struct Cache {
    pub max_age: NonZeroU64,
    pub io: Box<IO>,
}

impl Cache {
    ///
    /// Wraps an expression with the cache primitive.
    /// Performance DFS on the cache on the expression and identifies all the IO
    /// nodes. Then wraps each IO node with the cache primitive.
    pub fn wrap(max_age: NonZeroU64, expr: IR) -> IR {
        expr.modify(&mut move |expr| match expr {
            IR::IO(io) => Some(IR::Cache(Cache { max_age, io: Box::new(io.to_owned()) })),
            _ => None,
        })
    }
}

impl IR {
    pub fn pipe(self, next: Self) -> Self {
        IR::Pipe(Box::new(self), Box::new(next))
    }

    pub fn modify<F: FnMut(&IR) -> Option<IR>>(self, modifier: &mut F) -> IR {
        self.modify_inner(modifier)
    }

    fn modify_box<F: FnMut(&IR) -> Option<IR>>(self, modifier: &mut F) -> Box<IR> {
        Box::new(self.modify_inner(modifier))
    }

    fn modify_inner<F: FnMut(&IR) -> Option<IR>>(self, modifier: &mut F) -> IR {
        let modified = modifier(&self);
        match modified {
            Some(expr) => expr,
            None => {
                let expr = self;
                match expr {
                    IR::Pipe(first, second) => {
                        IR::Pipe(first.modify_box(modifier), second.modify_box(modifier))
                    }
                    IR::ContextPath(path) => IR::ContextPath(path),
                    IR::Dynamic(_) => expr,
                    IR::IO(_) => expr,
                    IR::Cache(Cache { io, max_age }) => {
                        let expr = *IR::IO(*io).modify_box(modifier);
                        match expr {
                            IR::IO(io) => IR::Cache(Cache { io: Box::new(io), max_age }),
                            expr => expr,
                        }
                    }
                    IR::Path(expr, path) => IR::Path(expr.modify_box(modifier), path),
                    IR::Protect(auth, expr) => IR::Protect(auth, expr.modify_box(modifier)),
                    IR::Map(Map { input, map }) => {
                        IR::Map(Map { input: input.modify_box(modifier), map })
                    }
                    IR::Discriminate(discriminator, expr) => {
                        IR::Discriminate(discriminator, expr.modify_box(modifier))
                    }
                    IR::Entity(map) => IR::Entity(
                        map.into_iter()
                            .map(|(k, v)| (k, v.modify(modifier)))
                            .collect(),
                    ),
                    IR::Service(sdl) => IR::Service(sdl),
                    IR::Deferred { ir, path, id, label } => {
                        IR::Deferred { ir: Box::new(ir.modify(modifier)), path, id, label }
                    }
                }
            }
        }
    }
}

impl<'a, Ctx: ResolverContextLike + Sync> CacheKey<EvalContext<'a, Ctx>> for IO {
    fn cache_key(&self, ctx: &EvalContext<'a, Ctx>) -> Option<IoId> {
        match self {
            IO::Http { req_template, .. } => req_template.cache_key(ctx),
            IO::Grpc { req_template, .. } => req_template.cache_key(ctx),
            IO::GraphQL { req_template, .. } => req_template.cache_key(ctx),
            IO::Js { .. } => None,
        }
    }
}
