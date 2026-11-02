//! How many files a batch works on at once.
//!
//! The right amount of parallelism depends on what the data is read from and written to. A
//! server or an SSD is limited by round trips and command queues, so more files in flight is
//! faster; a spinning disk is limited by its head, so parallel files make it slower. The host
//! classifies the local sides with its platform adapter and calls [`tune`]; the engine then
//! starts at the chosen parallelism and, where a ceiling allows, probes upwards while the
//! measured throughput keeps rising ([`Tuner`]).

use crate::batch::BatchOptions;

/// What kind of storage a local folder lives on, as far as copying is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageClass {
    /// Flash storage with no seek penalty: internal SSD, NVMe, RAM disk.
    Solid,
    /// A spinning disk: parallel files make the head jump.
    Rotational,
    /// A removable drive such as a USB stick or card.
    Removable,
    /// A network share mapped to a drive letter.
    Network,
    /// The adapter could not tell.
    Unknown,
}

/// Where the data of one batch comes from and goes to.
#[derive(Debug, Clone, Default)]
pub struct Involves {
    /// Classes of the local sides (sources and destination); empty when none is local.
    pub local: Vec<StorageClass>,
    /// Whether a remote provider (an SFTP/S3/WebDAV drive) is one of the sides.
    pub remote: bool,
}

/// The starting parallelism and the ceiling for the tuner, for `involves`.
pub fn policy(involves: &Involves) -> (usize, usize, bool) {
    let has = |class: StorageClass| involves.local.contains(&class);
    if has(StorageClass::Rotational) {
        if involves.remote {
            // The network is the slower side, but the head still must not be thrashed.
            return (2, 4, true);
        }
        return (1, 1, true);
    }
    if involves.remote {
        // Over SFTP, 8 files at once was about twice as fast as 4, and 16 faster again.
        return (8, 16, false);
    }
    if has(StorageClass::Removable) {
        return (2, 6, false);
    }
    if has(StorageClass::Network) || has(StorageClass::Unknown) {
        return (4, 8, false);
    }
    // Flash with nothing slower involved: measured faster at 8 files than at 4 for small files.
    (8, 16, false)
}

/// `options` with parallelism, ceiling and ordering chosen for `involves`.
pub fn tune(mut options: BatchOptions, involves: &Involves) -> BatchOptions {
    let (start, ceiling, ordered) = policy(involves);
    options.max_concurrent = start;
    options.adaptive_ceiling = ceiling;
    options.ordered = ordered;
    options
}

/// A file costs about this many bytes of work even when it is empty (a create, a close and a
/// round trip), which is what makes thousands of small files slow. Throughput is measured in
/// bytes plus this much per file so that both big and small files register as progress.
pub const BYTES_PER_FILE: u64 = 64 * 1024;

/// Hill climbing on measured throughput. It raises parallelism while that pays off and goes
/// back to the last good value as soon as it does not, then stays put.
#[derive(Debug, Clone)]
pub struct Tuner {
    target: usize,
    ceiling: usize,
    probe: Option<Probe>,
    settled: bool,
}

#[derive(Debug, Clone, Copy)]
struct Probe {
    /// Parallelism before the increase being judged.
    from: usize,
    /// Throughput measured at `from`.
    rate_before: f64,
}

/// An increase must beat the previous rate by this much to count as an improvement.
const REQUIRED_GAIN: f64 = 1.10;
/// A gain this large means the resource is nowhere near saturated, so the next step doubles.
const DOUBLING_GAIN: f64 = 1.5;

impl Tuner {
    pub fn new(start: usize, ceiling: usize) -> Self {
        let start = start.max(1);
        let ceiling = ceiling.max(start);
        Self {
            target: start,
            ceiling,
            probe: None,
            settled: ceiling == start,
        }
    }

    pub fn target(&self) -> usize {
        self.target
    }

    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// The next parallelism to try: half as many again, or twice as many while every step is
    /// still paying off by at least `gain` of 1.5 (short jobs end before a slow climb helps).
    fn next(&self, gain: f64) -> usize {
        let step = if gain >= DOUBLING_GAIN {
            self.target
        } else {
            (self.target / 2).max(1)
        };
        (self.target + step).min(self.ceiling)
    }

    /// Feeds the throughput measured over the last interval (at the current target) and returns
    /// the new target when it changed.
    pub fn observe(&mut self, rate: f64) -> Option<usize> {
        if self.settled || rate <= 0.0 {
            return None;
        }
        match self.probe.take() {
            None => {
                self.probe = Some(Probe {
                    from: self.target,
                    rate_before: rate,
                });
                self.target = self.next(0.0);
                Some(self.target)
            }
            Some(probe) if rate >= probe.rate_before * REQUIRED_GAIN => {
                if self.target >= self.ceiling {
                    self.settled = true;
                    return None;
                }
                let gain = rate / probe.rate_before;
                self.probe = Some(Probe {
                    from: self.target,
                    rate_before: rate,
                });
                self.target = self.next(gain);
                Some(self.target)
            }
            Some(probe) => {
                self.settled = true;
                self.target = probe.from;
                Some(self.target)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn involves(local: &[StorageClass], remote: bool) -> Involves {
        Involves {
            local: local.to_vec(),
            remote,
        }
    }

    #[test]
    fn a_spinning_disk_gets_one_ordered_stream() {
        assert_eq!(
            policy(&involves(&[StorageClass::Rotational], false)),
            (1, 1, true)
        );
        assert_eq!(
            policy(&involves(
                &[StorageClass::Solid, StorageClass::Rotational],
                false
            )),
            (1, 1, true)
        );
    }

    #[test]
    fn solid_state_and_servers_get_room_to_grow() {
        assert_eq!(
            policy(&involves(
                &[StorageClass::Solid, StorageClass::Solid],
                false
            )),
            (8, 16, false)
        );
        assert_eq!(
            policy(&involves(&[StorageClass::Solid], true)),
            (8, 16, false)
        );
        assert_eq!(policy(&involves(&[], true)), (8, 16, false));
    }

    #[test]
    fn a_disk_behind_a_server_is_limited_but_not_serialised() {
        assert_eq!(
            policy(&involves(&[StorageClass::Rotational], true)),
            (2, 4, true)
        );
    }

    #[test]
    fn removable_network_and_unknown_drives_are_careful() {
        assert_eq!(
            policy(&involves(&[StorageClass::Removable], false)),
            (2, 6, false)
        );
        assert_eq!(
            policy(&involves(&[StorageClass::Network], false)),
            (4, 8, false)
        );
        assert_eq!(
            policy(&involves(&[StorageClass::Unknown], false)),
            (4, 8, false)
        );
    }

    #[test]
    fn tuning_sets_the_options_the_engine_reads() {
        let tuned = tune(
            BatchOptions::default(),
            &involves(&[StorageClass::Rotational], false),
        );

        assert_eq!(tuned.max_concurrent, 1);
        assert_eq!(tuned.adaptive_ceiling, 1);
        assert!(tuned.ordered);
    }

    /// Throughput of a resource that saturates at `limit` parallel files.
    fn saturating(limit: usize) -> impl Fn(usize) -> f64 {
        move |parallel| (parallel.min(limit) * 100) as f64
    }

    fn climb(start: usize, ceiling: usize, throughput: impl Fn(usize) -> f64) -> Tuner {
        let mut tuner = Tuner::new(start, ceiling);
        for _ in 0..50 {
            let rate = throughput(tuner.target());
            tuner.observe(rate);
        }
        tuner
    }

    #[test]
    fn climbs_while_throughput_rises_and_stops_near_saturation() {
        let tuner = climb(4, 16, saturating(10));

        assert!(tuner.is_settled());
        assert!(
            (10..=16).contains(&tuner.target()),
            "settled at {}",
            tuner.target()
        );
    }

    #[test]
    fn goes_back_to_the_start_when_more_parallelism_does_not_help() {
        let tuner = climb(4, 16, |_| 500.0);

        assert!(tuner.is_settled());
        assert_eq!(tuner.target(), 4);
    }

    #[test]
    fn backs_off_when_parallelism_makes_things_worse() {
        let tuner = climb(4, 16, |parallel| if parallel > 4 { 300.0 } else { 400.0 });

        assert_eq!(tuner.target(), 4);
    }

    #[test]
    fn never_exceeds_the_ceiling() {
        let tuner = climb(4, 8, |parallel| (parallel * 100) as f64);

        assert_eq!(tuner.target(), 8);
        assert!(tuner.is_settled());
    }

    #[test]
    fn a_fixed_ceiling_never_probes() {
        let mut tuner = Tuner::new(4, 4);

        assert!(tuner.is_settled());
        assert_eq!(tuner.observe(1000.0), None);
        assert_eq!(tuner.target(), 4);
    }

    #[test]
    fn idle_intervals_change_nothing() {
        let mut tuner = Tuner::new(2, 8);

        assert_eq!(tuner.observe(0.0), None);
        assert_eq!(tuner.target(), 2);
        assert!(!tuner.is_settled());
    }
}
