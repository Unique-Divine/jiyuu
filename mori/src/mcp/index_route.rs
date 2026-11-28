//! Choose the update route for an existing generation using C-owned proofs.

use std::ffi::{c_int, c_void};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
#[cfg_attr(not(test), allow(dead_code, reason = "C supplies the plan result"))]
enum Plan {
    Declined = 0,
    Ready = 1,
    Fatal = 2,
}

#[derive(Debug, PartialEq, Eq)]
#[repr(C)]
enum Route {
    Noop = 0,
    Closure = 1,
    FullMode = 2,
    FullDeclined = 3,
    Fatal = 4,
}

type Exact = unsafe extern "C" fn(*mut c_void) -> bool;
type TryPlan = unsafe extern "C" fn(*mut c_void) -> Plan;

type RouteAction = unsafe extern "C" fn(*mut c_void) -> c_int;

unsafe extern "C" fn route_existing(
    proof: *mut c_void,
    live_pending: bool,
    exact: Exact,
    noop: RouteAction,
    staged: RouteAction,
) -> c_int {
    // SAFETY: C lends the live proof context and its matching callbacks.
    let action = if live_pending && unsafe { exact(proof) } {
        noop
    } else {
        staged
    };
    // SAFETY: the selected action operates on the same live context.
    unsafe { action(proof) }
}

unsafe extern "C" fn choose_staged(
    proof: *mut c_void,
    force_full_on_mismatch: bool,
    exact: Exact,
    plan: TryPlan,
) -> Route {
    // SAFETY: C lends these matching callbacks and the staged proof context.
    // Exact proof populates metadata the closure planner consumes.
    if unsafe { exact(proof) } {
        return Route::Noop;
    }
    if force_full_on_mismatch {
        return Route::FullMode;
    }
    // SAFETY: exact proof has run once, and the same context is still live.
    match unsafe { plan(proof) } {
        Plan::Ready => Route::Closure,
        Plan::Declined => Route::FullDeclined,
        Plan::Fatal => Route::Fatal,
    }
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_pipeline_set_index_route(
        live: unsafe extern "C" fn(
            *mut c_void,
            bool,
            Exact,
            RouteAction,
            RouteAction,
        ) -> c_int,
        staged: unsafe extern "C" fn(*mut c_void, bool, Exact, TryPlan) -> Route,
    );
}

#[cfg(not(test))]
pub(crate) unsafe fn register() {
    if std::env::var_os("MORI_IMPL_INDEX_ROUTE")
        .is_none_or(|selector| selector == "rust")
    {
        // SAFETY: startup registration precedes all C workers. Both
        // callbacks stay valid for the process lifetime.
        unsafe { cbm_pipeline_set_index_route(route_existing, choose_staged) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Proof {
        exact: bool,
        planner_result: Plan,
        exact_calls: u8,
        plan_calls: u8,
    }

    unsafe extern "C" fn exact(proof: *mut c_void) -> bool {
        // SAFETY: the test passes a live, exclusively borrowed Proof.
        let proof = unsafe { &mut *proof.cast::<Proof>() };
        proof.exact_calls += 1;
        proof.exact
    }

    unsafe extern "C" fn plan(proof: *mut c_void) -> Plan {
        // SAFETY: the test passes a live, exclusively borrowed Proof.
        let proof = unsafe { &mut *proof.cast::<Proof>() };
        proof.plan_calls += 1;
        if proof.exact_calls == 1 {
            proof.planner_result
        } else {
            Plan::Fatal
        }
    }

    unsafe extern "C" fn noop(_: *mut c_void) -> c_int {
        0
    }

    unsafe extern "C" fn staged(_: *mut c_void) -> c_int {
        -2
    }

    #[test]
    fn live_route_calls_only_the_selected_action_and_returns_its_status() {
        for (live_pending, is_exact, expected, exact_calls) in [
            (true, true, 0, 1),
            (true, false, -2, 1),
            (false, true, -2, 0),
        ] {
            let mut proof = Proof {
                exact: is_exact,
                planner_result: Plan::Declined,
                exact_calls: 0,
                plan_calls: 0,
            };
            // SAFETY: the callbacks accept this live Proof.
            let rc = unsafe {
                route_existing(
                    (&raw mut proof).cast(),
                    live_pending,
                    exact,
                    noop,
                    staged,
                )
            };
            assert_eq!(rc, expected);
            assert_eq!(proof.exact_calls, exact_calls);
        }
    }

    #[test]
    fn staged_route_orders_proofs_and_preserves_fatal_results() {
        for (is_exact, force_full, planner_result, expected, plan_calls) in [
            (true, true, Plan::Fatal, Route::Noop, 0),
            (false, true, Plan::Ready, Route::FullMode, 0),
            (false, false, Plan::Ready, Route::Closure, 1),
            (false, false, Plan::Declined, Route::FullDeclined, 1),
            (false, false, Plan::Fatal, Route::Fatal, 1),
        ] {
            let mut proof = Proof {
                exact: is_exact,
                planner_result,
                exact_calls: 0,
                plan_calls: 0,
            };
            // SAFETY: both callbacks read only this live Proof.
            let route = unsafe {
                choose_staged((&raw mut proof).cast(), force_full, exact, plan)
            };
            assert_eq!(route, expected);
            assert_eq!((proof.exact_calls, proof.plan_calls), (1, plan_calls));
        }
    }
}
