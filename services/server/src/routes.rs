//! The server's routes, registered through one builder that also writes down what it registers, so
//! the list of routes can be compared with the OpenAPI files (docs/openapi/client.v1.yaml and
//! server.v1.yaml) and cannot drift from them. axum cannot list a router's routes by itself.

use crate::AppState;
use axum::{
    handler::Handler,
    routing::{any, delete, get, post, put},
    Router,
};
use std::sync::Arc;

/// One registered route. `method` is `GET`, `POST`, `PUT`, `DELETE`, or `ANY`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteEntry {
    pub method: String,
    pub path: String,
}

pub struct Routes {
    router: Router<Arc<AppState>>,
    table: Vec<RouteEntry>,
}

impl Routes {
    pub fn new() -> Routes {
        Routes { router: Router::new(), table: Vec::new() }
    }

    fn add(mut self, method: &str, path: &str, route: axum::routing::MethodRouter<Arc<AppState>>) -> Routes {
        self.router = self.router.route(path, route);
        self.table.push(RouteEntry { method: method.into(), path: path.into() });
        self
    }

    pub fn get<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AppState>>,
        T: 'static,
    {
        self.add("GET", path, get(h))
    }

    pub fn post<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AppState>>,
        T: 'static,
    {
        self.add("POST", path, post(h))
    }

    pub fn put<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AppState>>,
        T: 'static,
    {
        self.add("PUT", path, put(h))
    }

    pub fn delete<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AppState>>,
        T: 'static,
    {
        self.add("DELETE", path, delete(h))
    }

    /// Every method. Used for the one route that tells an old app to update.
    pub fn any<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AppState>>,
        T: 'static,
    {
        self.add("ANY", path, any(h))
    }

    /// Applies `f` (a `route_layer`) to everything registered so far; what is added later is not covered.
    pub fn guarded(mut self, f: impl FnOnce(Router<Arc<AppState>>) -> Router<Arc<AppState>>) -> Routes {
        self.router = f(self.router);
        self
    }

    pub fn merge(mut self, other: Routes) -> Routes {
        self.router = self.router.merge(other.router);
        self.table.extend(other.table);
        self
    }

    pub fn into_parts(self) -> (Router<Arc<AppState>>, Vec<RouteEntry>) {
        (self.router, self.table)
    }
}

impl Default for Routes {
    fn default() -> Self {
        Routes::new()
    }
}

fn throwaway_state() -> Arc<AppState> {
    let db = crate::Db::open_in_memory().expect("an in-memory database opens");
    AppState::new(db, crate::MIN_CLIENT)
}

/// Every route the client API registers (including the one `ANY` for the old prefix), sorted.
pub fn client_routes() -> Vec<RouteEntry> {
    let (_, mut table) = crate::client_routes(&throwaway_state()).into_parts();
    table.sort();
    table
}

/// Every route the server API registers, sorted.
pub fn server_routes() -> Vec<RouteEntry> {
    let (_, mut table) = crate::server_api::routes().into_parts();
    table.sort();
    table
}
