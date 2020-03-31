//! [`tracing`] filters.
//!
//! This module provides a set of filters for instrumenting Warp applications
//! with [`tracing`] spans. [Spans] can be used to associate individual events
//! with a request, and track contexts through the application.
//!
//! This module requires the "trace" feature flag.
//!
//! [`tracing`]: https://crates.io/tracing
/// [Spans]: https://docs.rs/tracing/latest/tracing/#spans
use http::header;
use std::net::SocketAddr;

use crate::filter::{Filter, WrapSealed};
use crate::reject::IsReject;
use crate::reply::Reply;
use crate::route::Route;

use self::internal::WithTrace;

/// Create a wrapping filter that instruments every request with a `tracing`
/// [`Span`] provided by a function.
///
/// # Example
///
/// ```
/// use warp::Filter;
///
/// let route = warp::any()
///     .map(warp::reply)
///     .with(warp::trace(|req| {
///         tracing::info_span!("request", path = ?req.path()))
///     });
/// ```
///
/// [`Span`]: https://docs.rs/tracing/latest/tracing/#spans
pub fn trace<F>(func: F) -> Trace<F>
where
    F: Fn(Info<'_>) -> tracing::Span + Clone + Send,
{
    Trace { func }
}

/// Create a wrapping filter that instruments every request with a `tracing`
/// [`Span`] at the `INFO` level, containing a summary of the request.
///
/// # Example
///
/// ```
/// use warp::Filter;
///
/// let route = warp::any()
///     .map(warp::reply)
///     .with(warp::trace::request());
/// ```
///
/// [`Span`]: https://docs.rs/tracing/latest/tracing/#spans
pub fn request() -> Trace {
    trace(|route: Info| {
        tracing::info_span!(
            target: "warp",
            "request",
            method = %route.method(),
            path = ?route.path(),
            version = ?route.version(),
        )
    })
}

/// Create a wrapping filter that instruments every request with a `tracing`
/// [`Span`] at the `DEBUG` level representing a named context.
///
/// This can be used to instrument multiple routes with their own sub-spans in a
/// per-request trace.
///
/// # Example
///
/// ```
/// use warp::Filter;
///
/// let hello = warp::path("hello")
///     .map(warp::reply)
///     .with(warp::trace::context("hello"));
///
/// let goodbye = warp::path("goodbye")
///     .map(warp::reply)
///     .with(warp::trace::context("goodbye"));
///
/// let routes = hello.or(goodbye);
/// ```
///
/// [`Span`]: https://docs.rs/tracing/latest/tracing/#spans
pub fn context(name: &'static str) -> Trace<impl Fn(Info<'_>) -> tracing::Span + Copy> {
    trace(move |_| {
        tracing::debug_span!(
            target: "warp",
            "context",
            "{}", name,
        )
    })
}

/// Decorates a [`Filter`](crate::Filter) to create a [`tracing`] span for
/// requests and responses.
///
/// [`tracing`]: https://crates.io/tracing
#[derive(Clone, Copy, Debug)]
pub struct Trace<F = fn(Info) -> tracing::Span> {
    func: F,
}

/// Information about a request.
#[allow(missing_debug_implementations)]
pub struct Info<'a> {
    route: &'a Route,
}

impl<'a> Info<'a> {
    /// View the remote `SocketAddr` of the request.
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.route.remote_addr()
    }

    /// View the `http::Method` of the request.
    pub fn method(&self) -> &http::Method {
        self.route.method()
    }

    /// View the URI path of the request.
    pub fn path(&self) -> &str {
        self.route.full_path()
    }

    /// View the `http::Version` of the request.
    pub fn version(&self) -> http::Version {
        self.route.version()
    }

    /// View the referer of the request.
    pub fn referer(&self) -> Option<&str> {
        self.route
            .headers()
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok())
    }

    /// View the user agent of the request.
    pub fn user_agent(&self) -> Option<&str> {
        self.route
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
    }

    /// View the host of the request
    pub fn host(&self) -> Option<&str> {
        self.route
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
    }

    /// Access the full headers of the request
    pub fn headers(&self) -> &http::HeaderMap {
        self.route.headers()
    }
}

impl<FN, F> WrapSealed<F> for Trace<FN>
where
    FN: Fn(Info) -> tracing::Span + Clone + Send,
    F: Filter + Clone + Send,
    F::Extract: Reply,
    F::Error: IsReject,
{
    type Wrapped = WithTrace<FN, F>;

    fn wrap(&self, filter: F) -> Self::Wrapped {
        WithTrace {
            filter,
            trace: self.clone(),
        }
    }
}

mod internal {
    use futures::{future, FutureExt};
    use tracing_futures::{Instrument, Instrumented};

    use super::{Info, Trace};
    use crate::filter::{Filter, FilterBase, Internal};
    use crate::reject::IsReject;
    use crate::reply::{Reply, Response};
    use crate::route;

    #[derive(Clone, Copy)]
    #[allow(missing_debug_implementations)]
    pub struct WithTrace<FN, F> {
        pub(super) filter: F,
        pub(super) trace: Trace<FN>,
    }

    impl<FN, F> FilterBase for WithTrace<FN, F>
    where
        FN: Fn(Info<'_>) -> tracing::Span + Clone + Send,
        F: Filter + Clone + Send,
        F::Extract: Reply,
        F::Error: IsReject,
    {
        type Extract = (Traced,);
        type Error = F::Error;
        type Future = Instrumented<
            future::Map<F::Future, fn(Result<F::Extract, F::Error>) -> Result<(Traced,), F::Error>>,
        >;

        fn filter(&self, _: Internal) -> Self::Future {
            let span = route::with(|route| (self.trace.func)(Info { route }));
            span.in_scope(|| {
                tracing::trace!("received request");
                self.filter.filter(Internal)
            })
            .map(Traced::map_result as fn(_) -> _)
            .instrument(span)
        }
    }

    #[allow(missing_debug_implementations)]
    pub struct Traced(pub(super) Response);

    impl Reply for Traced {
        #[inline]
        fn into_response(self) -> Response {
            self.0
        }
    }

    impl Traced {
        fn map_result<R, E>(res: Result<R, E>) -> Result<(Traced,), E>
        where
            R: Reply,
            E: IsReject,
        {
            match res {
                Ok(reply) => {
                    let resp = reply.into_response();
                    tracing::debug!(response.status = resp.status().as_u16());
                    Ok((Traced(resp),))
                }
                Err(reject) => {
                    tracing::trace!(
                        response.status = reject.status().as_u16(),
                        response.error = ?reject,
                    );
                    Err(reject)
                }
            }
        }
    }
}
