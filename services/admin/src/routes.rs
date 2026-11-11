//! The admin's routes, registered through one builder that also writes down what it registers, so
//! the list of routes the server has can be compared with the OpenAPI file (docs/openapi/admin.v1.yaml)
//! and cannot drift from it. axum cannot list a router's routes by itself.

use crate::AdminState;
use axum::{
    handler::Handler,
    http::Method,
    middleware,
    routing::{delete, get, post, put},
    Router,
};
use std::sync::Arc;

/// One registered route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteEntry {
    pub method: String,
    pub path: String,
}

pub struct Routes {
    router: Router<Arc<AdminState>>,
    table: Vec<RouteEntry>,
}

impl Routes {
    pub fn new() -> Routes {
        Routes { router: Router::new(), table: Vec::new() }
    }

    fn add(mut self, method: Method, path: &str, router: impl FnOnce(Router<Arc<AdminState>>) -> Router<Arc<AdminState>>) -> Routes {
        self.router = router(self.router);
        self.table.push(RouteEntry { method: method.to_string(), path: path.to_string() });
        self
    }

    pub fn get<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AdminState>>,
        T: 'static,
    {
        let p = path.to_string();
        self.add(Method::GET, path, move |r| r.route(&p, get(h)))
    }

    pub fn post<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AdminState>>,
        T: 'static,
    {
        let p = path.to_string();
        self.add(Method::POST, path, move |r| r.route(&p, post(h)))
    }

    pub fn put<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AdminState>>,
        T: 'static,
    {
        let p = path.to_string();
        self.add(Method::PUT, path, move |r| r.route(&p, put(h)))
    }

    pub fn delete<H, T>(self, path: &str, h: H) -> Routes
    where
        H: Handler<T, Arc<AdminState>>,
        T: 'static,
    {
        let p = path.to_string();
        self.add(Method::DELETE, path, move |r| r.route(&p, delete(h)))
    }

    /// Everything registered so far needs the person's own session (cookie); what is added later does not.
    /// An API key is not enough here.
    pub fn signed_in(mut self, state: &Arc<AdminState>) -> Routes {
        self.router = self.router.route_layer(middleware::from_fn_with_state(state.clone(), crate::web::require_session));
        self
    }

    /// Everything registered so far needs the session or an admin API key.
    pub fn signed_in_or_key(mut self, state: &Arc<AdminState>) -> Routes {
        self.router = self.router.route_layer(middleware::from_fn_with_state(state.clone(), crate::web::require_admin_auth));
        self
    }

    /// Everything registered so far that changes something is recorded before it is done, and refused when its record
    /// cannot be appended (`actions`). Put it before the sign-in layer, so only a request that is let in is recorded.
    pub fn audited(mut self, state: &Arc<AdminState>) -> Routes {
        self.router = self.router.route_layer(middleware::from_fn_with_state(state.clone(), crate::actions::record_changes));
        self
    }

    /// Everything registered so far, being open, still wants the browser header on a change.
    pub fn browser_header(mut self) -> Routes {
        self.router = self.router.route_layer(middleware::from_fn(crate::web::require_admin_header));
        self
    }

    pub fn merge(mut self, other: Routes) -> Routes {
        self.router = self.router.merge(other.router);
        self.table.extend(other.table);
        self
    }

    pub fn into_parts(self) -> (Router<Arc<AdminState>>, Vec<RouteEntry>) {
        (self.router, self.table)
    }
}

impl Default for Routes {
    fn default() -> Self {
        Routes::new()
    }
}

/// Every route the admin registers, sorted. Built from a throwaway in-memory state.
pub fn route_table() -> Vec<RouteEntry> {
    let state = AdminState::new(crate::AdminDb::open_in_memory().expect("an in-memory database opens"));
    let (_, mut table) = crate::routes(&state).into_parts();
    table.sort();
    table
}

/// The routes of the API only (under `/api/admin/v1`), sorted: what the OpenAPI file describes.
pub fn api_route_table() -> Vec<RouteEntry> {
    route_table().into_iter().filter(|r| r.path.starts_with(askbar_proto::paths::ADMIN_V1)).collect()
}
