use crate::{
    model::*,
    store::{Store, err},
};
use rusqlite::params;
use std::{
    fs::{File, OpenOptions},
    os::unix::{
        fs::OpenOptionsExt,
        io::{AsRawFd, RawFd},
    },
    path::Path,
    time::{Duration, Instant},
};

pub struct Observation {
    pub available: u64,
    pub domain: String,
}
pub struct Window {
    pub id: String,
    pub before: u64,
    pub domain: String,
    source: CapacitySource,
    samples: Samples,
}

pub fn observe(path: &Path) -> Result<Observation> {
    let source = CapacitySource::open(path)?;
    Ok(Observation {
        available: source.available()?,
        domain: source.domain,
    })
}

/// Keep the same mounted filesystem open for the whole accounting window. This
/// avoids resolving a path again after mutation and runs diskutil only once,
/// rather than launching a process for every capacity sample.
struct CapacitySource {
    file: File,
    domain: String,
}

impl CapacitySource {
    fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(err)?;
        let stat = filesystem_stat(&file)?;
        Ok(Self {
            file,
            domain: storage_domain(&stat)?,
        })
    }

    fn available(&self) -> Result<u64> {
        let stat = filesystem_stat(&self.file)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_bsize as u64))
    }

    fn sample(&self) -> Result<Samples> {
        let mut values = [Sample {
            available: 0,
            at: Instant::now(),
        }; 3];
        for (index, value) in values.iter_mut().enumerate() {
            if index != 0 {
                std::thread::sleep(Duration::from_millis(40));
            }
            *value = Sample {
                available: self.available()?,
                at: Instant::now(),
            };
        }
        Ok(Samples(values))
    }
}

fn filesystem_stat(file: &File) -> Result<libc::statfs> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { stat.assume_init() })
}

fn storage_domain(stat: &libc::statfs) -> Result<String> {
    #[cfg(target_os = "macos")]
    let domain = {
        let mount =
            unsafe { std::ffi::CStr::from_ptr(stat.f_mntonname.as_ptr()) }.to_string_lossy();
        let fs = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) }.to_string_lossy();
        if fs == "apfs" {
            let mut command = std::process::Command::new("/usr/sbin/diskutil");
            command.args(["info", "-plist", &mount]);
            let output = crate::probe::run(command, Duration::from_secs(2), 256 * 1024, None)
                .map_err(|reason| {
                    format!("Storage container identity is unavailable; no credit. {reason}")
                })?;
            if !output.status.success() {
                return Err("Storage container identity is unavailable; no credit.".into());
            }
            let xml = String::from_utf8_lossy(&output.stdout);
            let container = plist_string(&xml, "APFSContainerReference")
                .ok_or("Unknown APFS container; no credit.")?;
            // The boot session plus kernel container reference names the shared pool.
            // Windows never survive restart, so no later observation can reuse this domain.
            format!("apfs:{container}")
        } else {
            let dev =
                unsafe { std::ffi::CStr::from_ptr(stat.f_mntfromname.as_ptr()) }.to_string_lossy();
            format!("{fs}:{dev}")
        }
    };
    #[cfg(not(target_os = "macos"))]
    let domain = format!("fs:{}:{:?}", stat.f_type, stat.f_fsid);
    Ok(domain)
}

#[derive(Clone, Copy)]
struct Sample {
    available: u64,
    at: Instant,
}

struct Samples([Sample; 3]);

impl Samples {
    fn maximum(&self) -> u64 {
        self.0.iter().map(|sample| sample.available).max().unwrap()
    }

    fn minimum(&self) -> u64 {
        self.0.iter().map(|sample| sample.available).min().unwrap()
    }

    /// Reserve the largest measured positive ambient rate for the entire
    /// observation interval. Ordinary competing writes need no extra deduction:
    /// they already reduce the minimum observed recovery. No percentage or
    /// byte-for-byte stability threshold is involved.
    fn ambient_growth(&self, interval: Duration) -> Result<u64> {
        let mut reserve = 0;
        for pair in self.0.windows(2) {
            let elapsed = pair[1]
                .at
                .checked_duration_since(pair[0].at)
                .filter(|elapsed| !elapsed.is_zero())
                .ok_or("Capacity sample timing was invalid.")?;
            let growth = pair[1].available.saturating_sub(pair[0].available);
            let projected = u128::from(growth)
                .checked_mul(interval.as_nanos())
                .and_then(|numerator| numerator.checked_add(elapsed.as_nanos() - 1))
                .map(|numerator| numerator / elapsed.as_nanos())
                .unwrap_or(u128::MAX);
            reserve = reserve.max(u64::try_from(projected).unwrap_or(u64::MAX));
        }
        Ok(reserve)
    }
}

struct Budget {
    after: u64,
    observed: u64,
    ambient: u64,
}

fn recovery_budget(before: &Samples, after: &Samples) -> Result<Budget> {
    let interval = after.0[2]
        .at
        .checked_duration_since(before.0[0].at)
        .filter(|_| after.0[0].at >= before.0[2].at)
        .ok_or("Capacity samples did not surround the cleanup.")?;
    let after_bytes = after.minimum();
    Ok(Budget {
        after: after_bytes,
        observed: after_bytes.saturating_sub(before.maximum()),
        ambient: before
            .ambient_growth(interval)?
            .max(after.ambient_growth(interval)?),
    })
}

#[cfg(target_os = "macos")]
fn plist_string(xml: &str, key: &str) -> Option<String> {
    let tail = xml.split_once(&format!("<key>{key}</key>"))?.1.trim_start();
    let value = tail.strip_prefix("<string>")?.split_once("</string>")?.0;
    if value.is_empty()
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_/".contains(&c))
    {
        None
    } else {
        Some(value.into())
    }
}

/// Fresh, descriptor-based private allocation; unknown support is explicitly absent.
pub fn private_bytes(fd: RawFd) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct AttrList {
            count: u16,
            reserved: u16,
            common: u32,
            volume: u32,
            dir: u32,
            file: u32,
            extended: u32,
        }
        unsafe extern "C" {
            fn fgetattrlist(
                fd: libc::c_int,
                attrs: *mut AttrList,
                buf: *mut libc::c_void,
                size: usize,
                options: libc::c_ulong,
            ) -> libc::c_int;
        }
        let mut attrs = AttrList {
            count: 5,
            reserved: 0,
            common: 0x8000_0000,
            volume: 0,
            dir: 0,
            file: 0,
            extended: 8,
        };
        let mut buf = [0u8; 32];
        if unsafe { fgetattrlist(fd, &mut attrs, buf.as_mut_ptr().cast(), buf.len(), 0x20) } != 0 {
            return None;
        }
        let u32at = |offset| u32::from_ne_bytes(buf[offset..offset + 4].try_into().unwrap());
        if u32at(0) != 32 || u32at(4) & 0x8000_0000 == 0 || u32at(20) & 8 == 0 {
            return None;
        }
        let bytes = i64::from_ne_bytes(buf[24..32].try_into().unwrap());
        (bytes >= 0).then_some(bytes as u64)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = fd;
        None
    }
}

/// Observe private allocation for a captured single-link regular file without
/// opening it. The caller retains LocalOnlyIo and the private recovery directory,
/// and brackets this with full metadata checks: the attribute API's allocation
/// size field is not equivalent to stat on macOS.
/// Missing capabilities permit the descriptor fallback; changed metadata does not.
pub(crate) fn private_bytes_at(
    parent: RawFd,
    name: &std::ffi::CStr,
    expected: &crate::safety::EntryMeta,
) -> Result<Option<u64>> {
    #[cfg(target_os = "macos")]
    {
        if !expected.is_file() || expected.links != 1 || expected.is_dataless() {
            return Err("Private allocation requires a captured local single-link file.".into());
        }
        if name.to_bytes().is_empty()
            || name.to_bytes().contains(&b'/')
            || name == c"."
            || name == c".."
        {
            return Err("Private allocation requires one captured filename.".into());
        }
        unsafe extern "C" {
            fn getattrlistat(
                fd: libc::c_int,
                path: *const libc::c_char,
                attrs: *mut libc::attrlist,
                buffer: *mut libc::c_void,
                size: usize,
                options: libc::c_ulong,
            ) -> libc::c_int;
        }
        let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
        attrs.bitmapcount = 5;
        attrs.commonattr = PRIVATE_COMMON_ATTRIBUTES;
        attrs.fileattr = PRIVATE_FILE_ATTRIBUTES;
        attrs.forkattr = PRIVATE_EXTENDED_ATTRIBUTES;
        // Aligned backing; individual packed fields are decoded as byte slices.
        let mut buffer = [0u64; PRIVATE_METADATA_BYTES / 8];
        if unsafe {
            getattrlistat(
                parent,
                name.as_ptr(),
                &mut attrs,
                buffer.as_mut_ptr().cast(),
                PRIVATE_METADATA_BYTES,
                0x21, // FSOPT_NOFOLLOW | FSOPT_ATTR_CMN_EXTENDED
            )
        } != 0
        {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error().is_some_and(|code| {
                [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOSYS, libc::EINVAL].contains(&code)
            }) {
                Ok(None)
            } else {
                Err(format!(
                    "Cannot inspect captured private allocation: {error}"
                ))
            };
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), PRIVATE_METADATA_BYTES)
        };
        parse_private_metadata(bytes, expected)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (parent, name, expected);
        Ok(None)
    }
}

// sys/attr.h and xnu/bsd/vfs/vfs_attrlist.c: request only attributes obtained
// from vnode metadata. ATTR_FILE_ALLOCSIZE also reads a resource fork and has
// different semantics from stat's st_blocks; full stat checks remain mandatory.
#[cfg(target_os = "macos")]
const PRIVATE_COMMON_ATTRIBUTES: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_CHGTIME
    | libc::ATTR_CMN_OWNERID
    | libc::ATTR_CMN_ACCESSMASK
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;
#[cfg(target_os = "macos")]
const PRIVATE_FILE_ATTRIBUTES: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_DATALENGTH;
#[cfg(target_os = "macos")]
const PRIVATE_EXTENDED_ATTRIBUTES: u32 = 8; // ATTR_CMNEXT_PRIVATESIZE
#[cfg(target_os = "macos")]
const PRIVATE_METADATA_BYTES: usize = 104;

#[cfg(target_os = "macos")]
fn parse_private_metadata(
    bytes: &[u8],
    expected: &crate::safety::EntryMeta,
) -> Result<Option<u64>> {
    if bytes.len() < 24 {
        return Err("Truncated private allocation attributes.".into());
    }
    let word = |offset| u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let length = word(0) as usize;
    if length < 24 || length > bytes.len() {
        return Err("Invalid private allocation record length.".into());
    }
    let common = word(4);
    let file = word(16);
    let extended = word(20);
    if common & libc::ATTR_CMN_RETURNED_ATTRS == 0
        || common & !PRIVATE_COMMON_ATTRIBUTES != 0
        || word(8) != 0
        || word(12) != 0
        || file & !PRIVATE_FILE_ATTRIBUTES != 0
        || extended & !PRIVATE_EXTENDED_ATTRIBUTES != 0
    {
        return Err("Invalid private allocation attribute masks.".into());
    }
    // Without PACK_INVAL_ATTRS, omitted fields change subsequent offsets. Never
    // interpret an incomplete metadata layout as though all fields were returned.
    if common != PRIVATE_COMMON_ATTRIBUTES || file != PRIVATE_FILE_ATTRIBUTES {
        return Ok(None);
    }
    let has_private = extended == PRIVATE_EXTENDED_ATTRIBUTES;
    // Some filesystems retain the originally requested buffer length when the
    // private-size attribute is unavailable. Its mask, never trailing padding,
    // determines whether that value exists.
    if length != PRIVATE_METADATA_BYTES && (has_private || length != PRIVATE_METADATA_BYTES - 8) {
        return Err("Incomplete private allocation metadata.".into());
    }
    let wide = |offset| u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
    let modified_nanos = wide(40) as i64;
    let changed_nanos = wide(56) as i64;
    let size = wide(88) as i64;
    let links = u64::from(word(84));
    if !(0..1_000_000_000).contains(&modified_nanos)
        || !(0..1_000_000_000).contains(&changed_nanos)
        || size < 0
        || links == 0
    {
        return Err("Invalid captured file metadata.".into());
    }
    let identity = Identity {
        device: word(24) as libc::dev_t as u64,
        inode: wide(76),
        mode: (word(68) & 0xffff & !(libc::S_IFMT as u32)) | libc::S_IFREG as u32,
        size: size as u64,
        modified_ns: (wide(32) as i64)
            .saturating_mul(1_000_000_000)
            .saturating_add(modified_nanos),
        changed_ns: (wide(48) as i64)
            .saturating_mul(1_000_000_000)
            .saturating_add(changed_nanos),
    };
    // VREG is 1 (sys/vnode.h). The private value belongs to this exact metadata
    // observation; a mismatch must not be retried through a different pathname.
    if word(28) != 1
        || identity != expected.identity
        || links != expected.links
        || word(64) != expected.uid
        || word(72) != expected.flags
    {
        return Err("Captured file changed while inspecting private allocation.".into());
    }
    if !has_private {
        return Ok(None);
    }
    let private = wide(96) as i64;
    if private < 0 {
        return Err("Invalid private allocation size.".into());
    }
    Ok(Some(private as u64))
}

pub fn begin(store: &Store, path: &Path) -> Result<Window> {
    let source = CapacitySource::open(path)?;
    let samples = source.sample()?;
    let before = samples.maximum();
    let id = unique_id();
    store
        .conn
        .execute(
            "INSERT INTO windows(id,domain,before_bytes,state) VALUES(?1,?2,?3,'open')",
            params![id, source.domain, before],
        )
        .map_err(err)?;
    Ok(Window {
        id,
        before,
        domain: source.domain.clone(),
        source,
        samples,
    })
}

pub fn finish(
    store: &mut Store,
    window: Window,
    _path: &Path,
    receipt: &mut Receipt,
    private_bound: u64,
    private_known: bool,
) -> Result<()> {
    // The retained descriptor anchors these observations to the original
    // filesystem even if the caller's path is subsequently renamed or replaced.
    let budget = window
        .source
        .sample()
        .and_then(|after| recovery_budget(&window.samples, &after));
    let after = budget.as_ref().map_or(window.before, |budget| budget.after);
    let credit = supported_credit(receipt, private_bound, private_known, budget);
    allocate(store, &window.id, after, private_bound, credit, receipt)
}

fn supported_credit(
    receipt: &mut Receipt,
    private_bound: u64,
    private_known: bool,
    budget: Result<Budget>,
) -> u64 {
    let budget = match budget {
        Ok(budget) => budget,
        Err(reason) => {
            receipt.observed_bytes = 0;
            receipt.detail.push_str(&format!(
                " No space credited: storage observations were unavailable ({reason})."
            ));
            return 0;
        }
    };
    receipt.observed_bytes = budget.observed;
    let supported = budget.observed.saturating_sub(budget.ambient);
    let reason = if receipt.operation != "permanent" {
        Some("moving files to Trash does not establish recovered space")
    } else if receipt.outcome != "removed" {
        Some("cleanup did not finish removing all reviewed contents")
    } else if !private_known {
        Some("private allocation was unavailable for at least one removed file")
    } else if private_bound == 0 {
        Some("no private allocation was proven; the removed data may share storage")
    } else if receipt.reported_bytes == 0 {
        Some("the reviewed items had no allocated bytes")
    } else if budget.observed == 0 {
        Some("the capacity samples did not show an available-space increase")
    } else if supported == 0 {
        Some("measured background capacity growth accounts for the observed increase")
    } else {
        None
    };
    let credit = if let Some(reason) = reason {
        receipt
            .detail
            .push_str(&format!(" No space credited: {reason}."));
        0
    } else {
        let credit = private_bound.min(receipt.reported_bytes).min(supported);
        receipt.detail.push_str(&format!(
            " Credited {credit} bytes from conservative storage observations."
        ));
        credit
    };
    receipt.detail.push_str(&format!(
        " Accounting bounds: {} bytes observed, {} bytes reserved for measured background growth, {private_bound} private bytes, {} reviewed allocated bytes. APFS sharing, snapshots and other disk activity can reduce game credit; this is not an exact disk-savings claim.",
        budget.observed, budget.ambient, receipt.reported_bytes
    ));
    credit
}

/// One allocation per operation, one finite before/after budget per domain window.
fn allocate(
    store: &mut Store,
    window_id: &str,
    after: u64,
    private_bound: u64,
    credit: u64,
    receipt: &mut Receipt,
) -> Result<()> {
    let tx = store
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(err)?;
    let (state, before): (String, u64) = tx
        .query_row(
            "SELECT state,before_bytes FROM windows WHERE id=?1",
            [window_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(err)?;
    if state != "open" {
        return Err("This observation window has already been finalized.".into());
    }
    if credit > after.saturating_sub(before)
        || credit > private_bound
        || credit > receipt.reported_bytes
    {
        return Err("Credit exceeds its observation or item bound.".into());
    }
    let remainder: u64 = tx
        .query_row("SELECT remainder FROM wallet WHERE id=1", [], |r| r.get(0))
        .map_err(err)?;
    let accumulated = remainder.checked_add(credit).ok_or("Reward overflow")?;
    let coins = accumulated / COIN_BYTES;
    receipt.credited_bytes = credit;
    receipt.coins = coins;
    tx.execute(
        "INSERT INTO allocations VALUES(?1,?2,?3)",
        params![receipt.id, window_id, credit],
    )
    .map_err(err)?;
    tx.execute(
        "INSERT INTO earnings VALUES(?1,?2,0)",
        params![receipt.id, coins],
    )
    .map_err(err)?;
    tx.execute(
        "UPDATE wallet SET remainder=?1,credited=credited+?2 WHERE id=1",
        params![accumulated % COIN_BYTES, credit],
    )
    .map_err(err)?;
    tx.execute("UPDATE windows SET after_bytes=?2,private_bound=?3,credited_bytes=?4,state='closed' WHERE id=?1",params![window_id,after,private_bound,credit]).map_err(err)?;
    tx.execute(
        "UPDATE operations SET receipt_json=?2,state=?3 WHERE id=?1",
        params![
            receipt.id,
            serde_json::to_string(receipt).map_err(err)?,
            receipt.outcome
        ],
    )
    .map_err(err)?;
    tx.commit().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(id: &str, bound: u64) -> Receipt {
        Receipt {
            id: id.into(),
            path: "/fixture/target".into(),
            title: "Build artifacts".into(),
            operation: "permanent".into(),
            outcome: "removed".into(),
            detail: String::new(),
            created_at: now(),
            reported_bytes: bound,
            observed_bytes: bound,
            credited_bytes: 0,
            coins: 0,
            trash_path: None,
            can_restore: false,
            seq: None,
        }
    }

    fn setup(store: &Store, id: &str, window: &str, bound: u64) -> Receipt {
        let r = receipt(id, bound);
        store
            .conn
            .execute(
                "INSERT INTO operations VALUES(?1,'{}','{}',?2,NULL,NULL,'removed')",
                params![id, serde_json::to_string(&r).unwrap()],
            )
            .unwrap();
        store.conn.execute("INSERT INTO windows(id,domain,before_bytes,state) VALUES(?1,'test-domain',1000,'open')",[window]).unwrap();
        r
    }

    fn samples(start: Instant, values: [u64; 3]) -> Samples {
        Samples(std::array::from_fn(|index| Sample {
            available: values[index],
            at: start + Duration::from_millis(index as u64 * 40),
        }))
    }

    #[test]
    fn ordinary_background_writes_reduce_credit_instead_of_vetoing_it() {
        let start = Instant::now();
        let before = samples(start, [1_000_000_000, 999_000_000, 998_000_000]);
        let after = samples(
            start + Duration::from_secs(1),
            [1_331_000_000, 1_329_000_000, 1_327_000_000],
        );
        let mut r = receipt("busy", 331_000_000);
        let credit = supported_credit(&mut r, 331_000_000, true, recovery_budget(&before, &after));
        assert_eq!(r.observed_bytes, 327_000_000);
        assert_eq!(credit, 327_000_000);
        assert!(r.detail.contains("0 bytes reserved"));
    }

    #[test]
    fn measured_positive_ambient_growth_is_reserved_for_the_entire_window() {
        let start = Instant::now();
        let before = samples(start, [1_000_000_000, 1_001_000_000, 1_002_000_000]);
        let after = samples(
            start + Duration::from_secs(1),
            [1_302_000_000, 1_303_000_000, 1_304_000_000],
        );
        let budget = recovery_budget(&before, &after).unwrap();
        assert_eq!(budget.observed, 300_000_000);
        // 1 MB / 40 ms, projected over the 1080 ms observation interval.
        assert_eq!(budget.ambient, 27_000_000);
        let mut r = receipt("drift", 331_000_000);
        assert_eq!(
            supported_credit(&mut r, 331_000_000, true, Ok(budget)),
            273_000_000
        );
    }

    #[test]
    fn shared_unknown_or_unobserved_allocation_cannot_earn_credit() {
        for (private, known, observed, reason) in [
            (0, true, 300_000_000, "no private allocation"),
            (
                300_000_000,
                false,
                300_000_000,
                "unavailable for at least one",
            ),
            (
                300_000_000,
                true,
                0,
                "did not show an available-space increase",
            ),
        ] {
            let mut r = receipt("unproven", 300_000_000);
            assert_eq!(
                supported_credit(
                    &mut r,
                    private,
                    known,
                    Ok(Budget {
                        after: observed,
                        observed,
                        ambient: 0,
                    })
                ),
                0
            );
            assert!(r.detail.contains(reason), "{}", r.detail);
        }
        let mut r = receipt("unavailable", 300_000_000);
        assert_eq!(
            supported_credit(&mut r, 300_000_000, true, Err("device offline".into())),
            0
        );
        assert_eq!(r.observed_bytes, 0);
        assert!(r.detail.contains("device offline"));
    }

    #[test]
    fn trash_and_incomplete_cleanup_remain_ineligible() {
        for (operation, outcome) in [("trash", "trashed"), ("permanent", "partial")] {
            let mut r = receipt("ineligible", 300_000_000);
            r.operation = operation.into();
            r.outcome = outcome.into();
            assert_eq!(
                supported_credit(
                    &mut r,
                    300_000_000,
                    true,
                    Ok(Budget {
                        after: 300_000_000,
                        observed: 300_000_000,
                        ambient: 0,
                    })
                ),
                0
            );
        }
    }

    #[test]
    fn item_bounds_cap_unrelated_capacity_gains() {
        for (reported, private, expected) in [(120, 180, 120), (180, 120, 120)] {
            let mut r = receipt("bounded", reported);
            assert_eq!(
                supported_credit(
                    &mut r,
                    private,
                    true,
                    Ok(Budget {
                        after: 900,
                        observed: 900,
                        ambient: 100,
                    })
                ),
                expected
            );
        }
    }

    #[test]
    fn ambient_growth_can_exhaust_the_observation_without_underflow() {
        let mut r = receipt("ambient", 300_000_000);
        assert_eq!(
            supported_credit(
                &mut r,
                300_000_000,
                true,
                Ok(Budget {
                    after: 300_000_000,
                    observed: 300_000_000,
                    ambient: u64::MAX,
                })
            ),
            0
        );
        assert!(r.detail.contains("background capacity growth accounts"));
    }

    #[test]
    fn invalid_observation_order_cannot_support_a_reward() {
        let start = Instant::now();
        let before = samples(start, [100, 100, 100]);
        let overlapping = samples(start + Duration::from_millis(20), [200, 200, 200]);
        assert!(recovery_budget(&before, &overlapping).is_err());
        let mut after = samples(start + Duration::from_secs(1), [200, 200, 200]);
        after.0[1].at = after.0[0].at;
        assert!(recovery_budget(&before, &after).is_err());
    }

    #[cfg(target_os = "macos")]
    fn private_metadata_fixture() -> ([u8; PRIVATE_METADATA_BYTES], crate::safety::EntryMeta) {
        let expected = crate::safety::EntryMeta {
            identity: Identity {
                device: 17,
                inode: 23,
                mode: libc::S_IFREG as u32 | 0o600,
                size: 8192,
                modified_ns: 50_000_000_007,
                changed_ns: 70_000_000_011,
            },
            allocated: 8192,
            links: 1,
            uid: 501,
            flags: 0,
        };
        let mut bytes = [0; PRIVATE_METADATA_BYTES];
        for (offset, value) in [
            (0, PRIVATE_METADATA_BYTES as u32),
            (4, PRIVATE_COMMON_ATTRIBUTES),
            (16, PRIVATE_FILE_ATTRIBUTES),
            (20, PRIVATE_EXTENDED_ATTRIBUTES),
            (24, 17),
            (28, 1),
            (64, 501),
            (68, 0o600),
            (84, 1),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (offset, value) in [
            (32, 50u64),
            (40, 7),
            (48, 70),
            (56, 11),
            (76, 23),
            (88, 8192),
            (96, 4096),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
        }
        (bytes, expected)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_metadata_requires_complete_supported_fields() {
        let (bytes, expected) = private_metadata_fixture();
        assert_eq!(
            parse_private_metadata(&bytes, &expected).unwrap(),
            Some(4096)
        );
        for (offset, mask) in [
            (4, PRIVATE_COMMON_ATTRIBUTES & !libc::ATTR_CMN_FILEID),
            (16, PRIVATE_FILE_ATTRIBUTES & !libc::ATTR_FILE_LINKCOUNT),
        ] {
            let mut missing = bytes;
            missing[offset..offset + 4].copy_from_slice(&mask.to_ne_bytes());
            assert_eq!(parse_private_metadata(&missing, &expected).unwrap(), None);
        }
        for length in [96u32, 104] {
            let mut missing_private = bytes;
            missing_private[0..4].copy_from_slice(&length.to_ne_bytes());
            missing_private[20..24].copy_from_slice(&0u32.to_ne_bytes());
            assert_eq!(
                parse_private_metadata(&missing_private[..length as usize], &expected).unwrap(),
                None
            );
            // Missing private-size support does not excuse a visible identity change.
            missing_private[76..84].copy_from_slice(&24u64.to_ne_bytes());
            assert!(
                parse_private_metadata(&missing_private[..length as usize], &expected).is_err()
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_metadata_rejects_malformed_records_and_changed_files() {
        let (bytes, expected) = private_metadata_fixture();
        for end in 0..bytes.len() {
            assert!(parse_private_metadata(&bytes[..end], &expected).is_err());
        }
        for length in [0u32, 23, 96, 103, 105] {
            let mut malformed = bytes;
            malformed[0..4].copy_from_slice(&length.to_ne_bytes());
            assert!(parse_private_metadata(&malformed, &expected).is_err());
        }
        for (offset, value) in [
            (4, PRIVATE_COMMON_ATTRIBUTES | libc::ATTR_CMN_NAME),
            (8, 1),
            (12, 1),
            (16, PRIVATE_FILE_ATTRIBUTES | libc::ATTR_FILE_ALLOCSIZE),
            (20, PRIVATE_EXTENDED_ATTRIBUTES | 1),
            (24, 18),
            (28, 2),
            (64, 502),
            (68, 0o644),
            (72, 0x4000_0000),
            (84, 0),
            (84, 2),
        ] {
            let mut changed = bytes;
            changed[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
            assert!(parse_private_metadata(&changed, &expected).is_err());
        }
        for (offset, value) in [
            (32, 51u64),
            (40, 8),
            (40, 1_000_000_000),
            (48, 71),
            (56, 12),
            (56, u64::MAX),
            (76, 24),
            (88, 8193),
            (88, u64::MAX),
            (96, u64::MAX),
        ] {
            let mut changed = bytes;
            changed[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
            assert!(parse_private_metadata(&changed, &expected).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn captured_private_allocation_matches_descriptors_for_local_files() {
        use std::{
            ffi::OsStr,
            io::{Seek, SeekFrom, Write},
            os::unix::ffi::OsStrExt,
        };

        let _local_io = crate::safety::LocalOnlyIo::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let parent = File::open(dir.path()).unwrap();
        for (name, sparse, resource_fork) in [
            (c"regular", false, false),
            (c"sparse", true, false),
            (c"resource-fork", false, true),
        ] {
            let path = dir.path().join(OsStr::from_bytes(name.to_bytes()));
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            if sparse {
                file.set_len(1024 * 1024).unwrap();
                file.seek(SeekFrom::Start(1024 * 1024 - 4096)).unwrap();
            }
            file.write_all(&[0x61; 4096]).unwrap();
            if resource_fork {
                let resource = [0x72u8; 8192];
                assert_eq!(
                    unsafe {
                        libc::fsetxattr(
                            file.as_raw_fd(),
                            c"com.apple.ResourceFork".as_ptr(),
                            resource.as_ptr().cast(),
                            resource.len(),
                            0,
                            0,
                        )
                    },
                    0,
                    "{}",
                    std::io::Error::last_os_error()
                );
            }
            file.sync_all().unwrap();
            let before = crate::safety::stat_fd(file.as_raw_fd()).unwrap();
            assert_eq!(
                private_bytes_at(parent.as_raw_fd(), name, &before).unwrap(),
                private_bytes(file.as_raw_fd()),
                "{name:?}"
            );
            assert_eq!(crate::safety::stat_fd(file.as_raw_fd()).unwrap(), before);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apfs_clone_blocks_are_not_private_allocation() {
        use std::{ffi::CString, io::Write, os::unix::ffi::OsStrExt};

        let _local_io = crate::safety::LocalOnlyIo::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        if !observe(dir.path()).unwrap().domain.starts_with("apfs:") {
            return;
        }
        let original_path = dir.path().join("original");
        let clone_path = dir.path().join("clone");
        let mut original = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&original_path)
            .unwrap();
        original.write_all(&vec![0x67; 4 * 1024 * 1024]).unwrap();
        original.sync_all().unwrap();
        assert_eq!(private_bytes(original.as_raw_fd()), Some(4 * 1024 * 1024));
        let parent = File::open(dir.path()).unwrap();
        assert_eq!(
            private_bytes_at(
                parent.as_raw_fd(),
                c"original",
                &crate::safety::stat_fd(original.as_raw_fd()).unwrap(),
            )
            .unwrap(),
            Some(4 * 1024 * 1024)
        );

        let source = CString::new(original_path.as_os_str().as_bytes()).unwrap();
        let destination = CString::new(clone_path.as_os_str().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0) },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        let clone = File::open(clone_path).unwrap();
        assert_eq!(private_bytes(original.as_raw_fd()), Some(0));
        assert_eq!(private_bytes(clone.as_raw_fd()), Some(0));
        for (name, file) in [(c"original", &original), (c"clone", &clone)] {
            assert_eq!(
                private_bytes_at(
                    parent.as_raw_fd(),
                    name,
                    &crate::safety::stat_fd(file.as_raw_fd()).unwrap(),
                )
                .unwrap(),
                Some(0)
            );
        }
    }

    #[test]
    fn fractions_collection_and_restart_are_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("ledger.db");
        let mut s = Store::open(&db).unwrap();
        let mut r = setup(&s, "a", "w1", 60_000_000);
        allocate(&mut s, "w1", 60_001_000, 60_000_000, 60_000_000, &mut r).unwrap();
        assert_eq!(s.wallet().unwrap().pending_coins, 0);
        assert!(allocate(&mut s, "w1", 60_001_000, 60_000_000, 60_000_000, &mut r).is_err());
        drop(s);
        let mut s = Store::open(&db).unwrap();
        s.reconcile().unwrap();
        let mut r = setup(&s, "b", "w2", 55_000_000);
        allocate(&mut s, "w2", 55_001_000, 55_000_000, 55_000_000, &mut r).unwrap();
        assert_eq!(s.wallet().unwrap().fractional_bytes, 15_000_000);
        assert_eq!(s.collect().unwrap(), (0, 1, 1));
        assert_eq!(s.collect().unwrap(), (1, 1, 0));
        drop(s);
        let s = Store::open(&db).unwrap();
        assert_eq!(s.wallet().unwrap().collected_coins, 1);
        assert_eq!(s.wallet().unwrap().pending_coins, 0);
    }
    #[test]
    fn overlapping_windows_and_excess_credit_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("db")).unwrap();
        let mut r = setup(&s, "a", "w", 50);
        assert!(s.conn.execute("INSERT INTO windows(id,domain,before_bytes,state) VALUES('w2','test-domain',0,'open')",[]).is_err());
        assert!(allocate(&mut s, "w", 1020, 50, 30, &mut r).is_err());
        assert_eq!(s.wallet().unwrap().credited_bytes, 0);
    }
    #[test]
    fn interrupted_window_never_uses_fresh_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("db")).unwrap();
        let mut r = setup(&s, "a", "w", 50);
        s.reconcile().unwrap();
        assert!(allocate(&mut s, "w", 2000, 50, 50, &mut r).is_err());
        assert_eq!(s.wallet().unwrap().credited_bytes, 0);
    }
}
