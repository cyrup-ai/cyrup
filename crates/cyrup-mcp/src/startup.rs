//! One build's startup connect pass, server by server (MCP-629, MCP-628).
//!
//! pi's built-in MCP extension gives every server a `ready` promise that settles once that server
//! has connected and registered its tools, or has failed (`startConnection`, `server.ready`,
//! `extensions/mcp/index.ts:546-560` @v1.0.4). The first prompt waits only on the `ready` of the
//! servers whose tools it declares (`waitForDirectServers`, `:1040-1058`), and a script only on the
//! servers it names (`pi.on("tool_call")`, `:1073-1089`).
//!
//! The build here is one future ([`crate::extension::InitTask`]), and its tools reach the model in
//! one batch at its commit, so every wait on it used to be a wait on its slowest server. This is
//! the per-server half:
//!
//! * the build names the servers it is **discovering** — the ones whose tools are not known until
//!   they answer (`needsDiscovery`, `init.ts:336-341`) — before it connects any of them
//!   ([`StartupPass::plan`]);
//! * as each of them settles, a server that declares direct tools has its catalogue written and
//!   **published** to the model at once ([`StartupPass::publish`]), and the server leaves the set
//!   ([`StartupPass::settle`]);
//! * a waiter names the servers it needs and waits for exactly those ([`StartupPass::wait_for`]).
//!   A server reached only through the `mcp` gateway is ready when the build is: the gateway reads
//!   the committed catalogue, so for such a server the build itself is what there is to wait for.
//!
//! A server whose tools the cache already holds is not in the set: they were registered before any
//! server was contacted, so there is nothing to wait for. pi has no cache, which is why it waits for
//! every server that declares tools.
//!
//! # A call through a server's own route (MCP-635)
//!
//! pi registers a server's tools from that server's own connection once it is ready
//! (`registerTools`, `extensions/mcp/index.ts:368` @v1.0.4) and calls through that connection
//! (`tools.ts:281-283`), so a call to a fast server is never held by a slow one. Here a direct call
//! goes through a [`crate::proxy::ProxyCtx`], which the commit installs once every server has
//! answered. The pass carries a context of its own over the generation's state as soon as the build
//! has created that state ([`StartupPass::state_built`]) — the same `McpState` the commit installs,
//! with the commit still to come — and [`StartupPass::route_for`] hands it to a call once the
//! call's own server has settled, unless that server failed to connect, whose answer (the backoff the
//! commit records) is the build's to give.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;

/// Where one build's startup pass is, as a value a waiter can test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupProgress {
    /// The servers the pass is discovering and has not settled yet. `None` until the build has
    /// decided which those are: before it has, any server may still be one of them.
    discovering: Option<BTreeSet<String>>,
    /// The build has ended — resolved, rejected or abandoned — and nothing will settle any more.
    finished: bool,
}

impl StartupProgress {
    /// The servers of `wanted` this pass may still be discovering: every one of them before the
    /// build has planned, none once it has finished.
    #[must_use]
    pub fn pending<'a>(&self, wanted: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        wanted
            .into_iter()
            .filter(|name| {
                self.discovering
                    .as_ref()
                    .is_none_or(|discovering| discovering.contains(*name))
            })
            .map(str::to_string)
            .collect()
    }

    /// What a wait for `servers`, and for the build on behalf of `through_build`, is still waiting
    /// for: the servers of `servers` this pass may still be discovering and, while `through_build`
    /// names any server and the build has not ended, every server the build is still discovering —
    /// or, before it has planned, the servers of `through_build` themselves.
    ///
    /// A server reached through the `mcp` gateway waits for the build, so what holds it up is
    /// whatever holds the build, whether or not that server is one of them.
    #[must_use]
    pub fn waiting_on(
        &self,
        servers: &BTreeSet<String>,
        through_build: &BTreeSet<String>,
    ) -> Vec<String> {
        let mut waiting = self.pending(servers.iter().map(String::as_str));
        if !through_build.is_empty() && !self.finished {
            match &self.discovering {
                Some(discovering) => waiting.extend(discovering.iter().cloned()),
                None => waiting.extend(through_build.iter().cloned()),
            }
        }
        waiting.sort();
        waiting.dedup();
        waiting
    }
}

/// What [`StartupPass::wait_for`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waited {
    /// Every server waited for has settled, and those that declare tools have published them.
    Settled,
    /// The bound passed first; these were still connecting.
    GaveUp(Vec<String>),
}

/// The surface sync the extension runs when a server's catalogue has been written.
type Publisher = Box<dyn Fn() + Send + Sync>;

/// What the extension builds a call's route from once the build has created the generation's state:
/// the proxy context the commit would install, or `None` when the generation is no longer current.
type Contextualizer =
    Box<dyn Fn(&Arc<crate::state::McpState>) -> Option<Arc<crate::proxy::ProxyCtx>> + Send + Sync>;

/// One build's startup pass, shared by the build that advances it and every wait on it.
pub struct StartupPass {
    progress: watch::Sender<StartupProgress>,
    /// [`crate::extension::McpExtension::sync_tool_surface`], bound to the generation this pass
    /// belongs to. Unset in a build that has no extension to publish to.
    publisher: OnceLock<Publisher>,
    /// Serialises the cache write and the surface sync of servers that settle at the same moment:
    /// `save_metadata_cache_with` is a read-modify-write of one file through one temporary name per
    /// process, and two surface syncs that interleave can each register against the other's stale
    /// view of what is already registered.
    publishing: Mutex<()>,
    /// Builds [`Self::context`] from the state, installed by the extension (MCP-635).
    contextualizer: OnceLock<Contextualizer>,
    /// The route a direct call takes to its own server before the commit (MCP-635): a proxy
    /// context over the state this build created.
    context: OnceLock<Arc<crate::proxy::ProxyCtx>>,
    /// The servers whose startup connect ended without a connection — failed, or needing
    /// authentication. A call to one of them waits for the build, which records why.
    unconnected: Mutex<BTreeSet<String>>,
}

impl Default for StartupPass {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupPass {
    /// A pass that has planned nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            progress: watch::Sender::new(StartupProgress::default()),
            publisher: OnceLock::new(),
            publishing: Mutex::new(()),
            contextualizer: OnceLock::new(),
            context: OnceLock::new(),
            unconnected: Mutex::new(BTreeSet::new()),
        }
    }

    /// Install what [`Self::state_built`] builds a call's route with. The first one wins.
    pub fn set_contextualizer(
        &self,
        contextualizer: impl Fn(&Arc<crate::state::McpState>) -> Option<Arc<crate::proxy::ProxyCtx>>
        + Send
        + Sync
        + 'static,
    ) {
        let _ = self.contextualizer.set(Box::new(contextualizer));
    }

    /// The build has created the generation's state, before it contacts any server: build the
    /// route a direct call takes before the commit.
    pub fn state_built(&self, state: &Arc<crate::state::McpState>) {
        if let Some(contextualize) = self.contextualizer.get()
            && let Some(context) = contextualize(state)
        {
            let _ = self.context.set(context);
        }
    }

    /// The route built by [`Self::state_built`], if there is one yet.
    #[must_use]
    pub fn context(&self) -> Option<Arc<crate::proxy::ProxyCtx>> {
        self.context.get().cloned()
    }

    /// `server`'s startup connect ended without a connection. Recorded before it settles.
    pub fn unconnected(&self, server: &str) {
        if let Ok(mut unconnected) = self.unconnected.lock() {
            unconnected.insert(server.to_string());
        }
    }

    /// Wait until `server` has settled, with no bound of its own, and answer the route a call to
    /// it takes before the commit (MCP-635).
    ///
    /// `None` when the pass ends first (the build has settled and its own answer stands), when the
    /// server's startup connect ended without a connection (the build records why, and that is
    /// the answer the call gets), or when the build created no route. Cancellation is the
    /// caller's: the future holds no resource.
    pub async fn route_for(&self, server: &str) -> Option<Arc<crate::proxy::ProxyCtx>> {
        let mut progress = self.progress.subscribe();
        let settled = progress
            .wait_for(|progress| progress.finished || progress.pending([server]).is_empty())
            .await
            .ok()
            .is_some_and(|progress| !progress.finished);
        if !settled {
            return None;
        }
        let unconnected = self
            .unconnected
            .lock()
            .map_or(true, |unconnected| unconnected.contains(server));
        if unconnected {
            return None;
        }
        self.context()
    }

    /// Install what [`Self::publish`] runs after a server's catalogue is written. The first one
    /// wins; a pass belongs to one generation.
    pub fn set_publisher(&self, publisher: impl Fn() + Send + Sync + 'static) {
        let _ = self.publisher.set(Box::new(publisher));
    }

    /// The pass as it is now.
    #[must_use]
    pub fn progress(&self) -> StartupProgress {
        self.progress.borrow().clone()
    }

    /// The build has decided which servers it discovers at startup.
    pub fn plan(&self, discovering: impl IntoIterator<Item = String>) {
        let discovering: BTreeSet<String> = discovering.into_iter().collect();
        self.progress.send_modify(|progress| {
            if !progress.finished {
                progress.discovering = Some(discovering);
            }
        });
    }

    /// Write a server's catalogue (`write`) and register its tools with the model, before the server
    /// leaves the set, so a waiter that sees it settled also sees its tools.
    pub fn publish(&self, write: impl FnOnce()) {
        let _serial = self
            .publishing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        write();
        if let Some(publisher) = self.publisher.get() {
            publisher();
        }
    }

    /// `server` has settled — connected, failed, or needs authentication.
    pub fn settle(&self, server: &str) {
        self.progress.send_if_modified(|progress| {
            progress
                .discovering
                .as_mut()
                .is_some_and(|discovering| discovering.remove(server))
        });
    }

    /// The build has ended: every wait on it ends too.
    pub fn finish(&self) {
        self.progress.send_if_modified(|progress| {
            let changed = !progress.finished;
            progress.finished = true;
            changed
        });
    }

    /// Wait until none of `servers` is still being discovered and, when `through_build` names any
    /// server, until the build has ended, for at most `bound`. A wait that gives up names what it
    /// was still waiting for ([`StartupProgress::waiting_on`]).
    ///
    /// Cancellation is the caller's: the future holds no resource, so dropping it ends the wait.
    pub async fn wait_for(
        &self,
        servers: &BTreeSet<String>,
        through_build: &BTreeSet<String>,
        bound: Duration,
    ) -> Waited {
        let mut progress = self.progress.subscribe();
        let names = || servers.iter().map(String::as_str);
        let ready = |progress: &StartupProgress| {
            progress.pending(names()).is_empty() && (through_build.is_empty() || progress.finished)
        };
        let waited = tokio::time::timeout(bound, progress.wait_for(ready)).await;
        match waited {
            // The sender lives in `self`, which this future borrows, so `Err` (closed) cannot occur;
            // read it as settled rather than invent a server that is still connecting.
            Ok(_) => Waited::Settled,
            Err(_) => Waited::GaveUp(self.progress.borrow().waiting_on(servers, through_build)),
        }
    }

    /// Whether `servers` and, for `through_build`, the build are all settled already.
    #[must_use]
    pub fn is_ready(&self, servers: &BTreeSet<String>, through_build: &BTreeSet<String>) -> bool {
        let progress = self.progress.borrow();
        progress
            .pending(servers.iter().map(String::as_str))
            .is_empty()
            && (through_build.is_empty() || progress.finished)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    /// Before the build has planned, any server may be one it discovers; after it has finished,
    /// none is.
    #[test]
    fn a_server_is_pending_until_the_plan_leaves_it_out_or_the_pass_settles_it() {
        let pass = StartupPass::new();
        assert_eq!(pass.progress().pending(["fast", "slow"]), ["fast", "slow"]);

        pass.plan(["fast".to_string(), "slow".to_string()]);
        assert_eq!(pass.progress().pending(["fast", "cached"]), ["fast"]);

        pass.settle("fast");
        assert!(pass.progress().pending(["fast", "cached"]).is_empty());
        assert_eq!(pass.progress().pending(["slow"]), ["slow"]);

        pass.finish();
        assert!(pass.progress().pending(["slow"]).is_empty());
    }

    /// A plan that arrives after the build ended does not resurrect it.
    #[test]
    fn a_finished_pass_stays_finished() {
        let pass = StartupPass::new();
        pass.finish();
        pass.plan(["late".to_string()]);
        assert!(pass.progress().pending(["late"]).is_empty());
    }

    /// The wait ends on the settle of the servers it names, not on the others.
    #[tokio::test(start_paused = true)]
    async fn a_wait_ends_when_its_own_servers_settle_while_another_is_still_connecting() {
        let pass = Arc::new(StartupPass::new());
        pass.plan(["fast".to_string(), "slow".to_string()]);
        let settler = {
            let pass = Arc::clone(&pass);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                pass.settle("fast");
            })
        };

        let started = tokio::time::Instant::now();
        let waited = pass
            .wait_for(&names(&["fast"]), &BTreeSet::new(), Duration::from_secs(10))
            .await;

        assert_eq!(waited, Waited::Settled);
        assert_eq!(started.elapsed(), Duration::from_millis(300));
        settler.await.unwrap();
    }

    /// At its bound the wait gives up, naming the servers it was still waiting for — and only
    /// those.
    #[tokio::test(start_paused = true)]
    async fn a_wait_that_reaches_its_bound_names_what_it_was_still_waiting_for() {
        let pass = StartupPass::new();
        pass.plan(["fast".to_string(), "slow".to_string(), "other".to_string()]);
        pass.settle("fast");

        let started = tokio::time::Instant::now();
        let waited = pass
            .wait_for(
                &names(&["fast", "slow"]),
                &BTreeSet::new(),
                Duration::from_secs(10),
            )
            .await;

        assert_eq!(waited, Waited::GaveUp(vec!["slow".to_string()]));
        assert_eq!(started.elapsed(), Duration::from_secs(10));
    }

    /// A build that ends — however it ends — releases every waiter at once.
    #[tokio::test(start_paused = true)]
    async fn the_end_of_the_build_ends_every_wait() {
        let pass = Arc::new(StartupPass::new());
        let finisher = {
            let pass = Arc::clone(&pass);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                pass.finish();
            })
        };
        let started = tokio::time::Instant::now();
        assert_eq!(
            pass.wait_for(
                &names(&["never-planned"]),
                &BTreeSet::new(),
                Duration::from_secs(10)
            )
            .await,
            Waited::Settled
        );
        assert_eq!(started.elapsed(), Duration::from_millis(40));
        finisher.await.unwrap();
    }

    /// A server reached only through the gateway is ready when the build is: its settle alone does
    /// not end the wait, the end of the build does.
    #[tokio::test(start_paused = true)]
    async fn a_gateway_server_is_waited_for_until_the_build_ends() {
        let pass = Arc::new(StartupPass::new());
        pass.plan(["gateway".to_string(), "other".to_string()]);
        let driver = {
            let pass = Arc::clone(&pass);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                pass.settle("gateway");
                tokio::time::sleep(Duration::from_millis(100)).await;
                pass.finish();
            })
        };
        let started = tokio::time::Instant::now();
        let waited = pass
            .wait_for(
                &BTreeSet::new(),
                &names(&["gateway"]),
                Duration::from_secs(10),
            )
            .await;
        assert_eq!(waited, Waited::Settled);
        assert_eq!(started.elapsed(), Duration::from_millis(200));
        driver.await.unwrap();

        let unfinished = StartupPass::new();
        unfinished.plan(["gateway".to_string()]);
        assert_eq!(
            unfinished
                .wait_for(
                    &BTreeSet::new(),
                    &names(&["gateway"]),
                    Duration::from_secs(1)
                )
                .await,
            Waited::GaveUp(vec!["gateway".to_string()])
        );
    }

    /// A gateway server's wait is the build's, so a wait that gives up names what holds the build:
    /// the servers it is still discovering, whether or not the gateway server is one of them. Before
    /// this a gateway server that had settled, or whose tools the cache held, gave up naming
    /// nothing, and so was neither noted nor remembered as waited out.
    #[tokio::test(start_paused = true)]
    async fn a_gateway_servers_wait_that_gives_up_names_what_holds_the_build() {
        let pass = StartupPass::new();
        pass.plan(["gateway".to_string(), "slow".to_string()]);
        pass.settle("gateway");
        assert_eq!(
            pass.wait_for(
                &BTreeSet::new(),
                &names(&["gateway", "cached"]),
                Duration::from_secs(1)
            )
            .await,
            Waited::GaveUp(vec!["slow".to_string()])
        );
        assert_eq!(
            pass.progress()
                .waiting_on(&names(&["gateway"]), &BTreeSet::new()),
            Vec::<String>::new(),
            "a server reached through its own tools waits only for itself"
        );

        let unplanned = StartupPass::new();
        assert_eq!(
            unplanned
                .progress()
                .waiting_on(&BTreeSet::new(), &names(&["gateway"])),
            ["gateway"],
            "before the plan, the server itself may be what the build is discovering"
        );
        pass.finish();
        assert!(
            pass.progress()
                .waiting_on(&names(&["slow"]), &names(&["gateway"]))
                .is_empty()
        );
    }

    /// A server is published before it leaves the set, so a waiter woken by its settle already
    /// sees its tools; and nothing waits for a server nobody named.
    #[tokio::test(start_paused = true)]
    async fn a_server_is_published_before_its_waiter_wakes() {
        let pass = Arc::new(StartupPass::new());
        let published = Arc::new(AtomicUsize::new(0));
        {
            let published = Arc::clone(&published);
            pass.set_publisher(move || {
                published.fetch_add(1, Ordering::SeqCst);
            });
        }
        pass.plan(["fast".to_string()]);
        let waiter = {
            let pass = Arc::clone(&pass);
            let published = Arc::clone(&published);
            tokio::spawn(async move {
                let waited = pass
                    .wait_for(&names(&["fast"]), &BTreeSet::new(), Duration::from_secs(10))
                    .await;
                (waited, published.load(Ordering::SeqCst))
            })
        };
        tokio::task::yield_now().await;

        let mut wrote = false;
        pass.publish(|| wrote = true);
        pass.settle("fast");

        assert!(wrote);
        assert_eq!(waiter.await.unwrap(), (Waited::Settled, 1));
        assert_eq!(
            pass.wait_for(&BTreeSet::new(), &BTreeSet::new(), Duration::from_secs(10))
                .await,
            Waited::Settled,
            "an empty wait is no wait"
        );
    }

    /// MCP-635 — a call gets the route to its own server once that server has settled, while the
    /// others are still being discovered; a server whose startup connect failed gets none (the
    /// build's answer is its), and neither does a call still waiting when the pass ends. A server
    /// the plan leaves out (its tools came from the cache) is routed at once. Red at `83daabf4`,
    /// where there was no route and a call waited for the commit.
    #[tokio::test(start_paused = true)]
    async fn a_call_gets_its_servers_route_once_that_server_settles_and_not_otherwise() {
        let (ctx, _env) = crate::proxy::testsupport::ctx_with(
            crate::config::McpConfig::default(),
            &[],
            &[],
            crate::proxy::testsupport::FakeEnv::default(),
        );
        let pass = Arc::new(StartupPass::new());
        assert!(pass.context().is_none(), "no route before the state exists");
        {
            let ctx = Arc::clone(&ctx);
            pass.set_contextualizer(move |_| Some(Arc::clone(&ctx)));
        }
        pass.state_built(&ctx.state);

        // Before the plan any server may be one the pass discovers.
        assert!(
            tokio::time::timeout(Duration::from_secs(1), pass.route_for("fast"))
                .await
                .is_err(),
            "a route before the plan"
        );

        pass.plan(["fast".to_string(), "slow".to_string(), "down".to_string()]);
        let driver = {
            let pass = Arc::clone(&pass);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                pass.unconnected("down");
                pass.settle("down");
                tokio::time::sleep(Duration::from_millis(100)).await;
                pass.settle("fast");
            })
        };
        let started = tokio::time::Instant::now();
        let route = tokio::time::timeout(Duration::from_secs(10), pass.route_for("fast"))
            .await
            .expect("the route is handed out when its server settles")
            .expect("a connected server has a route");
        assert!(Arc::ptr_eq(&route, &ctx));
        assert_eq!(started.elapsed(), Duration::from_millis(200));
        driver.await.unwrap();

        assert!(
            pass.route_for("down").await.is_none(),
            "a server that did not connect waits for the build's answer"
        );
        assert!(
            pass.route_for("cached").await.is_some(),
            "a server the plan leaves out is routed at once"
        );

        let waiter = {
            let pass = Arc::clone(&pass);
            tokio::spawn(async move { pass.route_for("slow").await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        pass.finish();
        assert!(
            waiter.await.unwrap().is_none(),
            "once the pass has ended the build's answer stands"
        );
    }

    /// A contextualizer that declines (a generation no longer current) leaves no route.
    #[test]
    fn a_declined_route_is_no_route() {
        let (ctx, _env) = crate::proxy::testsupport::ctx_with(
            crate::config::McpConfig::default(),
            &[],
            &[],
            crate::proxy::testsupport::FakeEnv::default(),
        );
        let pass = StartupPass::new();
        pass.set_contextualizer(|_| None);
        pass.state_built(&ctx.state);
        assert!(pass.context().is_none());
    }
}
