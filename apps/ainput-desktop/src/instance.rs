use std::mem::size_of;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_EVENT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};

const TARGET_PROCESS_NAME: &str = "ainput-desktop.exe";
const INSTANCE_REPLACE_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_SLICE_MS: u32 = 200;
const WAIT_OBJECT_0: WAIT_EVENT = WAIT_EVENT(0);
const WAIT_TIMEOUT: WAIT_EVENT = WAIT_EVENT(258);

pub(crate) fn replace_existing_instance() -> Result<()> {
    let current_pid = unsafe { GetCurrentProcessId() };
    let other_pids = enumerate_target_processes(current_pid)?;

    for pid in other_pids {
        terminate_process(pid)?;
    }

    Ok(())
}

fn enumerate_target_processes(current_pid: u32) -> Result<Vec<u32>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .context("create process snapshot")?;
    let snapshot = HandleGuard(snapshot);

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    let mut found = Vec::new();
    let first_ok = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
    if !first_ok {
        return Ok(found);
    }

    loop {
        let process_name = utf16_cstr_to_string(&entry.szExeFile);
        if process_name.eq_ignore_ascii_case(TARGET_PROCESS_NAME)
            && entry.th32ProcessID != current_pid
        {
            found.push(entry.th32ProcessID);
        }

        let next_ok = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
        if !next_ok {
            break;
        }
    }

    Ok(found)
}

fn terminate_process(pid: u32) -> Result<()> {
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) }
        .with_context(|| format!("open existing ainput process {pid}"))?;
    let handle = HandleGuard(handle);

    unsafe { TerminateProcess(handle.0, 0) }
        .with_context(|| format!("terminate existing ainput process {pid}"))?;

    let deadline = Instant::now() + INSTANCE_REPLACE_TIMEOUT;
    loop {
        let wait_ms = remaining_wait_slice(deadline);
        let status = unsafe { WaitForSingleObject(handle.0, wait_ms) };
        if status == WAIT_OBJECT_0 {
            return Ok(());
        }
        if status != WAIT_TIMEOUT {
            return Err(anyhow!(
                "wait existing ainput process {pid} exit failed: {:?}",
                status
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "existing ainput process {pid} did not exit within {:?}",
                INSTANCE_REPLACE_TIMEOUT
            ));
        }
    }
}

fn remaining_wait_slice(deadline: Instant) -> u32 {
    let remaining = deadline.saturating_duration_since(Instant::now());
    remaining.as_millis().min(WAIT_SLICE_MS as u128) as u32
}

fn utf16_cstr_to_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
