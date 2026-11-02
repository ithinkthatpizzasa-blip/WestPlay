//! Changes the game's GPS route as the player does on the game's map (Maps' search: go to a place, add a stop,
//! end the route): the app asks (the bridge's [`westplay_shared::RouteBlock`]), and on the game's main thread
//! (the end of a frame) the plugin gives the game's navigator its new waypoints, through the game's own
//! function, as the game's map screen does. The game then plans the route through them; they are its own
//! waypoints, on its map, changed there as any other.
//!
//! The game keeps them in its navigator (found as the host finds it, crates/host/src/route.rs): lists
//! (`array_t`: data at `+0x08`, count at `+0x10`) of 0x18-byte targets, `{0, -1, map node, 2}`:
//! - `+0x1F8`: the waypoints in route order, then, on a job, the job's destination;
//! - `+0x248`: the waypoints, in the order they were added;
//! - `+0x1D0`: given empty (the game fills it as it plans), `+0x220`: the places to avoid, kept;
//! - the routing mode at `+0x1C8`: a job's (1-4, 6, 7) is kept; otherwise waypoints alone are mode 5, and no
//!   route at all mode 0. Without a job, the last waypoint is where the route ends.
//!
//! Its "set targets" function (`navigator, mode, four lists`) copies the lists in and plans the route again;
//! a list passed as the navigator's own is left as it is. Map nodes come from the game's node table by
//! uid (the function the `goto` console command uses). The game allows ten waypoints.
//!
//! Worked out on ETS2 1.61.1.1 (Windows) and 1.57.2.7 (the Mac game, `navigation/macos.rs`: three lists, not four).
//! Every piece is found in the running game's code by its shape, not by address, and checked; anything not found,
//! or a route not as described, and nothing is done. Finding them, and giving the navigator its lists, are each OS's
//! (`navigation/windows.rs`, `navigation/macos.rs`); what the route becomes is shared.

use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use westplay_shared::{RouteAction, RouteAnswer};

use crate::{bridge, log};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use macos::{Game, MODE, ROUTE, give, locate, waypoints};
#[cfg(windows)]
pub(crate) use windows::Image;
#[cfg(windows)]
use windows::{Game, MODE, ROUTE, give, locate, waypoints};

/// The game's limit on waypoints.
const MAX_WAYPOINTS: u64 = 10;
/// More targets than a route can hold: a count past this is a misread.
const MAX_TARGETS: u64 = 64;

/// A target: `{u32 0, i32 -1, map node, u32 2 (a map node), padding}`.
type Target = [u8; 0x18];

/// The game's `array_t` as its functions read it.
#[repr(C)]
struct List {
    allocator: usize,
    data: *const Target,
    count: u64,
    capacity: u64,
}

impl List {
    /// A list of `targets` for the game to copy from, made with `allocator` (one of the navigator's lists').
    fn of(allocator: usize, targets: &[Target]) -> Self {
        List {
            allocator,
            data: if targets.is_empty() { std::ptr::null() } else { targets.as_ptr() },
            count: targets.len() as u64,
            capacity: targets.len() as u64,
        }
    }
}

static GAME: OnceLock<Option<Game>> = OnceLock::new();

/// Answers the app's route request, if there's a new one.
///
/// # Safety
/// On the game's main thread only (the end of a frame, where the game itself changes its route).
pub unsafe fn serve() {
    let Some(bridge) = bridge::get() else { return };
    let block = &bridge.route;
    let request = block.request.load(Ordering::Acquire);
    if request == block.answered.load(Ordering::Relaxed) {
        return;
    }
    let action = RouteAction::from_code(block.action.load(Ordering::Acquire));
    let node = block.node.load(Ordering::Acquire);
    let count = (block.via_count.load(Ordering::Acquire) as usize).min(block.vias.len());
    let vias: Vec<u64> = block.vias[..count].iter().map(|via| via.load(Ordering::Acquire)).collect();
    // SAFETY: on the game's main thread (`serve`'s contract), with pieces `locate` found and checked.
    let answer = match (GAME.get_or_init(|| unsafe { locate() }), action) {
        // SAFETY: as above: on the game's main thread, with `game` as `locate` found it.
        (Some(game), Some(action)) => unsafe { change(game, action, node, &vias) },
        (None, _) => RouteAnswer::Unsupported,
        (_, None) => RouteAnswer::Unexpected,
    };
    log!("route: {action:?} node {node:#x} via {vias:x?}: {answer:?}");
    block.answer.store(answer as i32, Ordering::Relaxed);
    block.answered.store(request, Ordering::Release);
}

/// Reads the navigator's route, plans the change ([`plan`]) and gives it the result.
///
/// # Safety
/// On the game's main thread, with `game` as `locate` found it.
unsafe fn change(game: &Game, action: RouteAction, uid: u64, vias: &[u64]) -> RouteAnswer {
    // SAFETY: the global, the navigator's offset and the game's functions were found in the game's own code by
    // their exact shape (`locate`); the core object is read only when set, and its lists only as `targets`
    // checks them. The lists given to the game live through its call, which copies them.
    unsafe {
        let core = *game.core;
        if core.is_null() {
            return RouteAnswer::Unexpected;
        }
        let navigator = core.add(game.navigator);
        let node = match action {
            RouteAction::End => None,
            RouteAction::AddStop | RouteAction::Go | RouteAction::Follow => match game.find_node(core, uid) {
                Some(node) => Some(target(node)),
                None => return RouteAnswer::NoSuchNode,
            },
        };
        let mut through = Vec::with_capacity(vias.len());
        for &via in vias {
            match game.find_node(core, via) {
                Some(node) => through.push(target(node)),
                None => return RouteAnswer::NoSuchNode,
            }
        }
        let mode = (navigator.add(MODE) as *const u32).read();
        let Some(route) = targets(navigator.add(ROUTE)) else { return RouteAnswer::Unexpected };
        let Some(waypoints) = waypoints(navigator, &route, mode) else { return RouteAnswer::Unexpected };
        let planned = match action {
            RouteAction::Follow => follow(mode, &through, node),
            _ => {
                // Where each is: a map node's position (x, z in 1/256 m, as the game keeps it: route.rs). Each
                // target's node is the game's own, from its navigator's lists or its node table (checked as read).
                let place = |target: &Target| {
                    let at = node_of(target) as *const i32;
                    [at.read_unaligned() as f32 / 256.0, at.add(2).read_unaligned() as f32 / 256.0]
                };
                let truck = bridge::get()
                    .and_then(|bridge| bridge.plugin.telemetry.read())
                    .map_or([0.0; 2], |telemetry| [telemetry.x as f32, telemetry.z as f32]);
                plan(action, mode, &route, &waypoints, node, &place, truck)
            }
        };
        match planned {
            Ok(planned) => give(game, navigator, &planned),
            Err(answer) => return answer,
        }
    }
    RouteAnswer::Done
}

/// A route as the navigator takes it: its mode, the waypoints in route order (and a job's destination last),
/// and the waypoints in the order they were added.
#[derive(Debug, PartialEq)]
struct Planned {
    mode: u32,
    route: Vec<Target>,
    waypoints: Vec<Target>,
}

/// The route after `action`, from the navigator's: its `mode`, `route` and `waypoints` (as [`Planned`]), and
/// the place asked for (`node`, for going or adding). A refusal when the game wouldn't allow it. A stop goes
/// where it adds the least way ([`least_detour`]), by where things are (`place`, x z) from the truck (`truck`).
fn plan(
    action: RouteAction,
    mode: u32,
    route: &[Target],
    waypoints: &[Target],
    node: Option<Target>,
    place: &dyn Fn(&Target) -> [f32; 2],
    truck: [f32; 2],
) -> Result<Planned, RouteAnswer> {
    let job = matches!(mode, 1..=4 | 6 | 7);
    // On a job the route is its waypoints and then the job's destination; anything else is a misread.
    let destination = match (job, route.split_last()) {
        (true, Some((destination, before))) if before.len() == waypoints.len() => Some(*destination),
        (true, _) => return Err(RouteAnswer::Unexpected),
        (false, _) => None,
    };
    let ordered = &route[..route.len() - destination.is_some() as usize];
    let with_destination = |stops: &[Target]| [stops, destination.as_slice()].concat();
    let keep = |route: Vec<Target>, waypoints: Vec<Target>| {
        let mode = match (job, waypoints.is_empty()) {
            (true, _) => mode,
            (false, true) => 0,
            (false, false) => 5,
        };
        Ok(Planned { mode, route, waypoints })
    };
    match (action, node) {
        (RouteAction::End, _) => keep(with_destination(&[]), Vec::new()),
        (RouteAction::Go, _) if job => Err(RouteAnswer::OnJob),
        (RouteAction::Go, Some(node)) => keep(vec![node], vec![node]),
        (RouteAction::AddStop, Some(node)) => {
            if waypoints.iter().any(|stop| node_of(stop) == node_of(&node)) {
                return Err(RouteAnswer::AlreadyThere);
            }
            if waypoints.len() as u64 >= MAX_WAYPOINTS {
                return Err(RouteAnswer::Full);
            }
            // Where it adds the least way, as the game's map puts a stop (owner 2026-10-21: D, then B, on the way
            // from A to F: A B D F). Before where the route ends: a job's destination, or without a job its last
            // waypoint (a stop went after it, and the route ended at the stop).
            let (stops, end) = match (job, ordered.split_last()) {
                (false, Some((end, stops))) => (stops, Some(end)),
                _ => (ordered, destination.as_ref()),
            };
            let at = least_detour(stops, end, &node, place, truck);
            let end = end.map(std::slice::from_ref).unwrap_or_default();
            keep([&stops[..at], &[node], &stops[at..], end].concat(), [waypoints, &[node]].concat())
        }
        (RouteAction::Go | RouteAction::AddStop, None) => Err(RouteAnswer::Unexpected),
        // Through its waypoints: `follow`.
        (RouteAction::Follow, _) => Err(RouteAnswer::Unexpected),
    }
}

/// Where among `stops` (in route order, before the route's `end`) `new` adds the least way: the index it goes
/// in at. Each gap's way, from the truck to the first stop, between stops and on to the end, measured
/// straight, against the same with `new` in it; the first gap of the least.
fn least_detour(
    stops: &[Target],
    end: Option<&Target>,
    new: &Target,
    place: &dyn Fn(&Target) -> [f32; 2],
    truck: [f32; 2],
) -> usize {
    let far = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let at = place(new);
    (0..=stops.len())
        .map(|gap| {
            let from = if gap == 0 { truck } else { place(&stops[gap - 1]) };
            let to = stops.get(gap).or(end).map(place);
            let added = far(from, at) + to.map_or(0.0, |to| far(at, to) - far(from, to));
            (gap, added)
        })
        .fold((stops.len(), f32::INFINITY), |best, (gap, added)| if added < best.1 { (gap, added) } else { best })
        .0
}

/// The route to `node` through `vias` (in order), each a waypoint the game's map shows, as the route the
/// player picked in WestPlay: not on a job (whose destination the game keeps).
fn follow(mode: u32, vias: &[Target], node: Option<Target>) -> Result<Planned, RouteAnswer> {
    if matches!(mode, 1..=4 | 6 | 7) {
        return Err(RouteAnswer::OnJob);
    }
    let Some(node) = node else { return Err(RouteAnswer::Unexpected) };
    if vias.len() as u64 >= MAX_WAYPOINTS {
        return Err(RouteAnswer::Full);
    }
    let route = [vias, &[node]].concat();
    Ok(Planned { mode: 5, route: route.clone(), waypoints: route })
}

/// A list's targets, if it reads as one.
///
/// # Safety
/// `list` points at a game `array_t` of targets (the navigator's).
unsafe fn targets(list: *const u8) -> Option<Vec<Target>> {
    // SAFETY: `list` is an `array_t` (the caller's contract); its data is read only when its count is
    // plausible and its data set.
    let targets = unsafe {
        let list = &*(list as *const List);
        if list.count > MAX_TARGETS || (list.count > 0 && list.data.is_null()) {
            return None;
        }
        std::slice::from_raw_parts(list.data, list.count as usize).to_vec()
    };
    targets.iter().all(|target| target[16..20] == [2, 0, 0, 0] && node_of(target) != 0).then_some(targets)
}

fn node_of(target: &Target) -> usize {
    usize::from_le_bytes(target[8..16].try_into().unwrap())
}

fn target(node: usize) -> Target {
    let mut target = [0u8; 0x18];
    target[4..8].copy_from_slice(&(-1i32).to_le_bytes());
    target[8..16].copy_from_slice(&node.to_le_bytes());
    target[16..20].copy_from_slice(&2u32.to_le_bytes());
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(targets: &[Target]) -> Vec<usize> {
        targets.iter().map(node_of).collect()
    }

    /// Where the tests' targets are: each node's number, read as east metres along a road (0x10 is 16 m on).
    fn on_a_line(target: &Target) -> [f32; 2] {
        [node_of(target) as f32, 0.0]
    }

    /// `plan` on a straight road, the truck at its start.
    fn plan(
        action: RouteAction,
        mode: u32,
        route: &[Target],
        waypoints: &[Target],
        node: Option<Target>,
    ) -> Result<Planned, RouteAnswer> {
        super::plan(action, mode, route, waypoints, node, &on_a_line, [0.0, 0.0])
    }

    #[test]
    fn adds_a_stop_before_a_jobs_destination_and_keeps_the_jobs_mode() {
        let (a, b, destination, new) = (target(0xa0), target(0xb0), target(0xd0), target(0xe0));
        let planned = plan(RouteAction::AddStop, 1, &[a, b, destination], &[b, a], Some(new)).unwrap();
        assert_eq!(planned.mode, 1);
        assert_eq!(nodes(&planned.route), [0xa0, 0xb0, 0xe0, 0xd0]);
        // The order they were added in, the new one last.
        assert_eq!(nodes(&planned.waypoints), [0xb0, 0xa0, 0xe0]);
        // A job with no stops yet.
        let planned = plan(RouteAction::AddStop, 1, &[destination], &[], Some(new)).unwrap();
        assert_eq!(nodes(&planned.route), [0xe0, 0xd0]);
    }

    #[test]
    fn without_a_job_a_stop_goes_before_where_the_route_ends() {
        let (a, new) = (target(0xa0), target(0xe0));
        // No route: the stop is the route (mode 5, the game's for waypoints alone).
        let planned = plan(RouteAction::AddStop, 0, &[], &[], Some(new)).unwrap();
        assert_eq!((planned.mode, nodes(&planned.route)), (5, vec![0xe0]));
        // The route ends at its last waypoint: the stop before it, wherever it is (here past it).
        let planned = plan(RouteAction::AddStop, 5, &[a], &[a], Some(new)).unwrap();
        assert_eq!(nodes(&planned.route), [0xe0, 0xa0]);
        // Go: that place alone.
        let planned = plan(RouteAction::Go, 5, &[a], &[a], Some(new)).unwrap();
        assert_eq!((planned.mode, nodes(&planned.route), nodes(&planned.waypoints)), (5, vec![0xe0], vec![0xe0]));
    }

    #[test]
    fn a_stop_goes_where_it_adds_the_least_way() {
        // From A (the truck, at 0) to F at 0x60, B at 0x20 and D at 0x40 on the way (owner 2026-10-21): D added,
        // then B: A B D F, not A D B F.
        let (b, d, f) = (target(0x20), target(0x40), target(0x60));
        let with_d = plan(RouteAction::AddStop, 5, &[f], &[f], Some(d)).unwrap();
        assert_eq!(nodes(&with_d.route), [0x40, 0x60]);
        let with_b = plan(RouteAction::AddStop, 5, &with_d.route, &with_d.waypoints, Some(b)).unwrap();
        assert_eq!(nodes(&with_b.route), [0x20, 0x40, 0x60]);
        // The order they were added in stays theirs.
        assert_eq!(nodes(&with_b.waypoints), [0x60, 0x40, 0x20]);
        // The same on a job: its destination last.
        let with_d = plan(RouteAction::AddStop, 1, &[f], &[], Some(d)).unwrap();
        let with_b = plan(RouteAction::AddStop, 1, &with_d.route, &with_d.waypoints, Some(b)).unwrap();
        assert_eq!(nodes(&with_b.route), [0x20, 0x40, 0x60]);
        // One off the way: between the stops it's nearest the way between.
        let off = |x: f32, z: f32| {
            move |target: &Target| match node_of(target) {
                0x99 => [x, z],
                node => [node as f32, 0.0],
            }
        };
        let side = target(0x99);
        let planned =
            super::plan(RouteAction::AddStop, 5, &with_b.route, &[], Some(side), &off(0x30 as f32, 10.0), [0.0; 2]);
        assert_eq!(nodes(&planned.unwrap().route), [0x20, 0x99, 0x40, 0x60]);
    }

    #[test]
    fn ends_a_route_or_a_jobs_stops() {
        let (a, destination) = (target(0xa0), target(0xd0));
        let planned = plan(RouteAction::End, 5, &[a], &[a], None).unwrap();
        assert_eq!(planned, Planned { mode: 0, route: vec![], waypoints: vec![] });
        // On a job, the way to its destination stays.
        let planned = plan(RouteAction::End, 2, &[a, destination], &[a], None).unwrap();
        assert_eq!((planned.mode, nodes(&planned.route), planned.waypoints.len()), (2, vec![0xd0], 0));
    }

    #[test]
    fn follows_a_route_through_its_waypoints_to_its_end() {
        let ([a, b, end], job) = ([target(0x10), target(0x20), target(0x30)], 1);
        let planned = follow(5, &[a, b], Some(end)).unwrap();
        assert_eq!(
            (planned.mode, nodes(&planned.route), nodes(&planned.waypoints)),
            (5, vec![0x10, 0x20, 0x30], vec![0x10, 0x20, 0x30])
        );
        assert_eq!(nodes(&follow(0, &[], Some(end)).unwrap().route), vec![0x30]);
        assert_eq!(follow(job, &[a], Some(end)), Err(RouteAnswer::OnJob));
    }

    #[test]
    fn refuses_what_the_game_wouldnt_allow() {
        let (a, destination, new) = (target(0xa0), target(0xd0), target(0xe0));
        // A job's destination stays the job's.
        assert_eq!(plan(RouteAction::Go, 1, &[destination], &[], Some(new)), Err(RouteAnswer::OnJob));
        assert_eq!(plan(RouteAction::AddStop, 5, &[a], &[a], Some(a)), Err(RouteAnswer::AlreadyThere));
        let ten: Vec<Target> = (1..=10).map(|i| target(i * 0x10)).collect();
        assert_eq!(plan(RouteAction::AddStop, 5, &ten, &ten, Some(new)), Err(RouteAnswer::Full));
        // A job's route not its stops and then its destination: misread, so nothing is done.
        assert_eq!(plan(RouteAction::AddStop, 1, &[a], &[a], Some(new)), Err(RouteAnswer::Unexpected));
    }

    #[test]
    fn a_target_is_a_map_node_as_the_game_writes_it() {
        // As read from the game: a waypoint at map node 0x2432d8e4660 (the last four bytes are padding).
        let mut seen = target(0x2432d8e4660);
        seen[20..24].copy_from_slice(&0xd4u32.to_le_bytes());
        assert_eq!(&seen[..20], &hex("00000000ffffffff60468e2d4302000002000000")[..]);
        assert_eq!(node_of(&seen), 0x2432d8e4660);
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }
}
