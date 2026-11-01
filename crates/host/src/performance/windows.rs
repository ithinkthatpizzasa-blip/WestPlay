//! The processes WestPlay runs on Windows (WebView2's, under the app), and how much of the processor each took.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use webview2_com::GetProcessExtendedInfosCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PROCESS_KIND, COREWEBVIEW2_PROCESS_KIND_BROWSER, COREWEBVIEW2_PROCESS_KIND_GPU,
    COREWEBVIEW2_PROCESS_KIND_RENDERER, ICoreWebView2Environment, ICoreWebView2Environment13,
    ICoreWebView2ProcessExtendedInfo, ICoreWebView2ProcessExtendedInfoCollection,
};
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::core::{BOOL, Interface, PWSTR};

use super::{Profile, descendants, host_of, page_part, part_of};

pub type Environment = ICoreWebView2Environment;

/// Asks WebView2 which of `profile`'s processes is which (it answers later, on this thread).
pub fn ask_parts(
    parts: &Rc<RefCell<HashMap<u32, &'static str>>>,
    cab_host: &str,
    profile: Profile,
    environment: &Environment,
) {
    // Older WebView2 runtimes can't say: their processes count as "other".
    let Ok(environment) = environment.cast::<ICoreWebView2Environment13>() else { return };
    let parts = parts.clone();
    let cab_host = cab_host.to_owned();
    let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(move |error, infos| {
        if let (Ok(()), Some(infos)) = (error, infos) {
            for (pid, part) in parts_of(&infos, profile, &cab_host) {
                parts.borrow_mut().insert(pid, part);
            }
        }
        Ok(())
    }));
    // SAFETY: the handler is a local for the call; WebView2 keeps its own reference to it.
    let _ = unsafe { environment.GetProcessExtendedInfos(&handler) };
}

/// Each process of a WebView2 profile, and the part of WestPlay it is.
fn parts_of(
    infos: &ICoreWebView2ProcessExtendedInfoCollection,
    profile: Profile,
    cab_host: &str,
) -> Vec<(u32, &'static str)> {
    let mut count = 0;
    // SAFETY: `count` is a local the call writes.
    if unsafe { infos.Count(&mut count) }.is_err() {
        return Vec::new();
    }
    (0..count)
        .filter_map(|index| {
            // SAFETY: an index below the collection's count.
            let info = unsafe { infos.GetValueAtIndex(index) }.ok()?;
            let (pid, kind) = process_of(&info)?;
            let part = match profile {
                Profile::Spotify => "Spotify",
                Profile::AppleMusic => "Apple Music",
                Profile::Cab => match kind {
                    COREWEBVIEW2_PROCESS_KIND_BROWSER => "browser",
                    COREWEBVIEW2_PROCESS_KIND_GPU => "GPU",
                    COREWEBVIEW2_PROCESS_KIND_RENDERER => page_part(&frame_hosts(&info), cab_host),
                    _ => "helpers",
                },
            };
            Some((pid, part))
        })
        .collect()
}

fn process_of(info: &ICoreWebView2ProcessExtendedInfo) -> Option<(u32, COREWEBVIEW2_PROCESS_KIND)> {
    // SAFETY: plain queries; `pid` and `kind` are locals the calls write.
    unsafe {
        let process = info.ProcessInfo().ok()?;
        let (mut pid, mut kind) = (0, COREWEBVIEW2_PROCESS_KIND::default());
        process.ProcessId(&mut pid).ok()?;
        process.Kind(&mut kind).ok()?;
        Some((pid as u32, kind))
    }
}

/// The hosts of the pages a renderer draws.
fn frame_hosts(info: &ICoreWebView2ProcessExtendedInfo) -> Vec<String> {
    let mut hosts = Vec::new();
    // SAFETY: plain queries of WebView2's own collection; the strings it writes are freed by `CoTaskMemPWSTR`.
    unsafe {
        let Ok(frames) = info.AssociatedFrameInfos() else { return hosts };
        let Ok(frames) = frames.GetIterator() else { return hosts };
        let mut more = BOOL::default();
        while frames.HasCurrent(&mut more).is_ok() && more.as_bool() {
            if let Ok(frame) = frames.GetCurrent() {
                let mut source = PWSTR::null();
                if frame.Source(&mut source).is_ok() {
                    hosts.extend(host_of(&webview2_com::CoTaskMemPWSTR::from(source).to_string()));
                }
            }
            if frames.MoveNext(&mut more).is_err() {
                break;
            }
        }
    }
    hosts
}

/// What each process is, by WestPlay's process tree and what WebView2 named.
pub struct Namer<'a> {
    ours: u32,
    tree: Vec<(u32, u32)>,
    browsers: Vec<u32>,
    named: &'a HashMap<u32, &'static str>,
}

impl<'a> Namer<'a> {
    pub fn new(named: &'a HashMap<u32, &'static str>) -> Self {
        Self { ours: std::process::id(), tree: process_tree(), browsers: crate::webview::host_browsers(), named }
    }

    pub fn part(&self, pid: u32) -> &'static str {
        part_of(pid, self.ours, &self.tree, self.named, &self.browsers)
    }
}

/// Every process's id and its parent's, as Windows lists them now.
fn process_tree() -> Vec<(u32, u32)> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    // SAFETY: a snapshot of the process list, walked with an entry sized as the calls require, then closed.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return parents };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    parents
}

/// WestPlay's processes: its own and every one started from it (WebView2's), at any depth.
pub fn processes() -> Vec<u32> {
    descendants(std::process::id(), &process_tree())
}

/// Each process's processor time so far (100 ns units); one that can't be opened is left out.
pub fn processor_times(pids: &[u32]) -> HashMap<u32, u64> {
    let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    pids.iter()
        .filter_map(|&pid| {
            // SAFETY: a handle opened for a plain query and closed after it; the times are locals it writes.
            unsafe {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
                let (mut created, mut exited, mut kernel, mut user) = Default::default();
                let read = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
                let _ = CloseHandle(process);
                read.ok()?;
                Some((pid, ticks(kernel) + ticks(user)))
            }
        })
        .collect()
}
