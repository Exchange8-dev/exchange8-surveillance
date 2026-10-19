// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Exchange8:Surveillance
// SPDX-License-Identifier: Apache-2.0

//! Exchange8:Surveillance core - market-abuse pattern detection.
//!
//! Pattern detectors over the trade/order event stream; alerts feed case
//! management (RFW surface for compliance review). The shell wires the
//! event consume + Sentin8:Func module execution + alert persistence.
//!
//! Design: `services/surveillance/DESIGN.md`.

/// A market-abuse pattern an alert may represent.
///
/// `Layering`/`MomentumIgnition`/`InsiderTrading`/`FrontRunning`/`ExcessiveCrossing`
/// are constructed by the shell-registered detector modules (Sentin8:Func
/// tenant rules + built-ins); the pure core implements the wash/spoofing/
/// marking-close detectors.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Wash trade - same beneficial owner both sides.
    WashTrade,
    /// Spoofing - orders placed then cancelled before execution, moving price.
    Spoofing,
    /// Layering - multiple price levels placed then pulled.
    Layering,
    /// Momentum ignition - initiating trades to trigger others' algos.
    MomentumIgnition,
    /// Insider-like trading - unusual size/price/timing ahead of news.
    InsiderTrading,
    /// Front-running - trading ahead of a client order.
    FrontRunning,
    /// Marking the close/end-of-day manipulation.
    MarkingTheClose,
    /// Excessive self-crossing (cross book) volume.
    ExcessiveCrossing,
}

impl Pattern {
    pub fn key(self) -> &'static str {
        match self {
            Pattern::WashTrade => "wash_trade",
            Pattern::Spoofing => "spoofing",
            Pattern::Layering => "layering",
            Pattern::MomentumIgnition => "momentum_ignition",
            Pattern::InsiderTrading => "insider_trading",
            Pattern::FrontRunning => "front_running",
            Pattern::MarkingTheClose => "marking_the_close",
            Pattern::ExcessiveCrossing => "excessive_crossing",
        }
    }

    /// Regulatory severity ranking (case triage).
    pub fn severity(self) -> u8 {
        match self {
            Pattern::WashTrade
            | Pattern::Spoofing
            | Pattern::Layering
            | Pattern::MomentumIgnition => 3,
            Pattern::InsiderTrading | Pattern::FrontRunning => 4,
            Pattern::MarkingTheClose | Pattern::ExcessiveCrossing => 2,
        }
    }
}

/// A normalized observation fed to the detectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Beneficial owner (account) behind the order.
    pub account_id: u64,
    pub instrument_id: u32,
    pub ts_us: u64,
    pub qty: u64,
    pub price_raw: i128,
    /// Side for orders/trades; crosses are both.
    pub is_buy: bool,
    /// True when the observation is a trade execution.
    pub is_trade: bool,
    /// True when both sides share the beneficial owner (cross).
    pub is_cross: bool,
}

/// A detector alert (one per detection).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub pattern: Pattern,
    pub instrument_id: u32,
    pub account_id: u64,
    /// Confidence score 0-100 (heuristic-weighted).
    pub confidence: u8,
    /// Supporting observations (ids/timestamps for the case file).
    pub evidence: Vec<Observation>,
}

/// Case-management state for an investigated alert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub case_id: u64,
    pub alert: Alert,
    pub state: CaseState,
}

/// Case workflow (RFW surface: compliance review).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseState {
    Open,
    Investigating,
    Closed { dismissed: bool },
}

// ---------------------------------------------------------------------------
// Detectors
// ---------------------------------------------------------------------------

/// Wash-trade detector: crosses where both sides share the beneficial owner.
///
/// the source exchange analysis parity: `wash_trade` flag exists on ME executions; this detector
/// counts repeated self-crosses per account+instrument and alerts above a
/// threshold (repeated self-crossing is the abuse signature, a single
/// accidental cross is not).
pub struct WashTradeDetector {
    /// Self-cross count per (account, instrument) in the window.
    counts: std::collections::HashMap<(u64, u32), u32>,
    /// Alert threshold.
    pub threshold: u32,
}

impl WashTradeDetector {
    pub fn new(threshold: u32) -> Self {
        WashTradeDetector {
            counts: std::collections::HashMap::new(),
            threshold,
        }
    }

    pub fn observe(&mut self, o: &Observation) -> Option<Alert> {
        if !o.is_cross || !o.is_trade {
            return None;
        }
        let key = (o.account_id, o.instrument_id);
        let c = self.counts.entry(key).or_insert(0);
        *c += 1;
        if *c == self.threshold {
            return Some(Alert {
                pattern: Pattern::WashTrade,
                instrument_id: o.instrument_id,
                account_id: o.account_id,
                confidence: 90,
                evidence: vec![*o],
            });
        }
        None
    }
}

/// Spoofing detector: order bursts cancelled without execution that move the
/// reference price. Operates on cancel observations trailing order placement.
pub struct SpoofingDetector {
    /// (account, instrument) → recent placed-but-unexecuted cancels.
    cancels: std::collections::HashMap<(u64, u32), Vec<Observation>>,
    /// Cancel ratio over `window_us` that alerts (e.g. 90% of 10+ orders).
    pub min_orders: u32,
    pub ratio_bps: u32,
}

impl SpoofingDetector {
    pub fn new(min_orders: u32, ratio_bps: u32) -> Self {
        SpoofingDetector {
            cancels: std::collections::HashMap::new(),
            min_orders,
            ratio_bps,
        }
    }

    /// Observe a cancelled order (placed, then cancelled before execution).
    /// `executed_orders` counts the account's executed orders in the same
    /// window for the ratio computation.
    pub fn observe_cancel(&mut self, o: &Observation, executed_orders: u32) -> Option<Alert> {
        let key = (o.account_id, o.instrument_id);
        let v = self.cancels.entry(key).or_default();
        v.push(*o);
        let total = v.len() as u32 + executed_orders;
        if total >= self.min_orders {
            let ratio = v.len() as u32 * 10_000 / total;
            if ratio >= self.ratio_bps {
                let evidence = v.clone();
                v.clear();
                // ratio is bps (0..=10_000) - compute in u32 to avoid overflow.
                let confidence = (ratio * 100 / 10_000).min(100) as u8;
                return Some(Alert {
                    pattern: Pattern::Spoofing,
                    instrument_id: o.instrument_id,
                    account_id: o.account_id,
                    confidence,
                    evidence,
                });
            }
        }
        None
    }
}

/// Marking-the-close detector: outsized executions in the final minutes.
pub struct MarkingCloseDetector {
    /// Last-regular-session-close time (window start), microseconds.
    pub window_start_us: u64,
    /// Window length before close (e.g. last 5 minutes).
    pub window_len_us: u64,
    /// Notional multiple of the session median that alerts.
    pub multiple: u32,
    /// Median trade notional for the instrument session.
    medians: std::collections::HashMap<u32, i128>,
}

impl MarkingCloseDetector {
    pub fn new(window_start_us: u64, window_len_us: u64, multiple: u32) -> Self {
        MarkingCloseDetector {
            window_start_us,
            window_len_us,
            multiple,
            medians: std::collections::HashMap::new(),
        }
    }

    /// Feed the session median trade notional per instrument (from stats).
    pub fn set_median(&mut self, instrument_id: u32, median_notional: i128) {
        self.medians.insert(instrument_id, median_notional);
    }

    pub fn observe(&mut self, o: &Observation) -> Option<Alert> {
        if !o.is_trade {
            return None;
        }
        let in_window =
            o.ts_us >= self.window_start_us && o.ts_us <= self.window_start_us + self.window_len_us;
        if !in_window {
            return None;
        }
        let median = self.medians.get(&o.instrument_id)?;
        let notional = o.qty as i128 * o.price_raw;
        if median > &0 && notional > median * self.multiple as i128 {
            return Some(Alert {
                pattern: Pattern::MarkingTheClose,
                instrument_id: o.instrument_id,
                account_id: o.account_id,
                confidence: 70,
                evidence: vec![*o],
            });
        }
        None
    }
}

/// Case manager: alerts → cases, workflow transitions.
pub struct CaseManager {
    cases: Vec<Case>,
    next_case_id: u64,
}

impl CaseManager {
    pub fn new() -> Self {
        CaseManager {
            cases: Vec::new(),
            next_case_id: 1,
        }
    }

    /// Open a case for an alert (idempotent per evidence signature - the
    /// shell dedupes by (pattern, instrument, account, day)).
    pub fn open(&mut self, alert: Alert) -> u64 {
        let id = self.next_case_id;
        self.next_case_id += 1;
        self.cases.push(Case {
            case_id: id,
            alert,
            state: CaseState::Open,
        });
        id
    }

    pub fn transition(&mut self, case_id: u64, to: CaseState) -> bool {
        if let Some(c) = self.cases.iter_mut().find(|c| c.case_id == case_id) {
            c.state = to;
            true
        } else {
            false
        }
    }

    pub fn cases(&self) -> &[Case] {
        &self.cases
    }

    /// Open cases sorted by severity (triage view).
    pub fn open_by_severity(&self) -> Vec<&Case> {
        let mut open: Vec<&Case> = self
            .cases
            .iter()
            .filter(|c| c.state == CaseState::Open)
            .collect();
        open.sort_by_key(|c| std::cmp::Reverse(c.alert.pattern.severity()));
        open
    }
}

impl Default for CaseManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(account: u64, cross: bool) -> Observation {
        Observation {
            account_id: account,
            instrument_id: 1,
            ts_us: 1_000,
            qty: 100,
            price_raw: 10_000,
            is_buy: true,
            is_trade: true,
            is_cross: cross,
        }
    }

    #[test]
    fn pattern_keys_and_severity() {
        assert_eq!(Pattern::WashTrade.key(), "wash_trade");
        assert_eq!(Pattern::FrontRunning.key(), "front_running");
        assert_eq!(Pattern::WashTrade.severity(), 3);
        assert_eq!(Pattern::InsiderTrading.severity(), 4);
        assert_eq!(Pattern::MarkingTheClose.severity(), 2);
    }

    #[test]
    fn wash_trade_repeated_crosses_alert() {
        let mut d = WashTradeDetector::new(3);
        // Two self-crosses: no alert yet.
        assert!(d.observe(&obs(1, true)).is_none());
        assert!(d.observe(&obs(1, true)).is_none());
        // Third: alert fires exactly once (at the threshold).
        let alert = d.observe(&obs(1, true)).unwrap();
        assert_eq!(alert.pattern, Pattern::WashTrade);
        assert_eq!(alert.confidence, 90);
        // Fourth: no more alerts (count passed the threshold).
        assert!(d.observe(&obs(1, true)).is_none());
    }

    #[test]
    fn wash_trade_ignores_non_cross() {
        let mut d = WashTradeDetector::new(1);
        assert!(d.observe(&obs(1, false)).is_none()); // regular trade
        let mut cancel = obs(1, false);
        cancel.is_trade = false;
        assert!(d.observe(&cancel).is_none());
    }

    #[test]
    fn spoofing_cancel_ratio_alerts() {
        // 10+ orders, 90% cancelled without execution.
        let mut d = SpoofingDetector::new(10, 9_000);
        for _ in 0..9 {
            let mut o = obs(1, false);
            o.is_trade = false;
            assert!(d.observe_cancel(&o, 0).is_none());
        }
        // The alerting call also pushes its cancel before the ratio check,
        // so the evidence carries all 10 cancelled orders.
        let mut o = obs(1, false);
        o.is_trade = false;
        let alert = d.observe_cancel(&o, 1).unwrap();
        assert_eq!(alert.pattern, Pattern::Spoofing);
        assert_eq!(alert.evidence.len(), 10);
        // Ratio 10 cancels / 11 total ≈ 90.9% → confidence 90.
        assert_eq!(alert.confidence, 90);
        // Detector reset after alerting.
        let mut o2 = obs(1, false);
        o2.is_trade = false;
        assert!(d.observe_cancel(&o2, 0).is_none());
    }

    #[test]
    fn spoofing_low_ratio_no_alert() {
        // Balanced trader: 5 cancels, 5 executions → 50% ratio, no alert.
        let mut d = SpoofingDetector::new(10, 9_000);
        for _ in 0..5 {
            let mut o = obs(1, false);
            o.is_trade = false;
            assert!(d.observe_cancel(&o, 5).is_none());
        }
    }

    #[test]
    fn marking_close_outsized_trade_alerts() {
        // Close window 10:00:00-10:05:00; median notional 100×10_000.
        let mut d = MarkingCloseDetector::new(3_600_000_000_000, 300_000_000_000, 5);
        d.set_median(1, 100 * 10_000);
        // Regular-sized trade in the window: no alert.
        assert!(d.observe(&obs(1, false)).is_none());
        // 10× the median notional at the close: alert.
        let mut big = obs(1, false);
        big.qty = 1_000; // 10× qty → 10× notional
        big.ts_us = 3_700_000_000_000;
        let alert = d.observe(&big).unwrap();
        assert_eq!(alert.pattern, Pattern::MarkingTheClose);
        // Outside the window: no alert.
        let mut late = big;
        late.ts_us = 4_500_000_000_000;
        assert!(d.observe(&late).is_none());
    }

    #[test]
    fn case_workflow_transitions() {
        let mut m = CaseManager::new();
        let alert = Alert {
            pattern: Pattern::WashTrade,
            instrument_id: 1,
            account_id: 7,
            confidence: 90,
            evidence: vec![obs(7, true)],
        };
        let id = m.open(alert.clone());
        assert_eq!(id, 1);
        assert_eq!(m.cases()[0].state, CaseState::Open);
        assert!(m.transition(id, CaseState::Investigating));
        assert!(m.transition(id, CaseState::Closed { dismissed: true }));
        assert_eq!(m.cases()[0].state, CaseState::Closed { dismissed: true });
        assert!(!m.transition(999, CaseState::Open));
    }

    #[test]
    fn triage_sorts_open_by_severity() {
        let mut m = CaseManager::new();
        let mk = |p: Pattern| Alert {
            pattern: p,
            instrument_id: 1,
            account_id: 7,
            confidence: 50,
            evidence: vec![],
        };
        // Open a low-severity then a high-severity case.
        m.open(mk(Pattern::ExcessiveCrossing)); // severity 2 → case 1
        m.open(mk(Pattern::FrontRunning)); // severity 4 → case 2
        let triage = m.open_by_severity();
        assert_eq!(triage.len(), 2);
        assert_eq!(triage[0].alert.pattern, Pattern::FrontRunning);
        // Closed cases drop out of triage.
        m.transition(2, CaseState::Closed { dismissed: false });
        assert_eq!(m.open_by_severity().len(), 1);
    }

    // -------------------------------------------------------------------
    // Untested-edge top-up - the untested edges of the pure detection core.
    // -------------------------------------------------------------------

    fn custom(
        account: u64,
        instrument: u32,
        ts: u64,
        qty: u64,
        cross: bool,
        is_trade: bool,
    ) -> Observation {
        Observation {
            account_id: account,
            instrument_id: instrument,
            ts_us: ts,
            qty,
            price_raw: 10_000,
            is_buy: true,
            is_trade,
            is_cross: cross,
        }
    }

    #[test]
    fn wash_trade_accounts_are_independent() {
        // Threshold 2: two different accounts each cross once - no alert
        // (per-(account, instrument) counters, never global).
        let mut d = WashTradeDetector::new(2);
        assert!(d.observe(&custom(1, 1, 1, 100, true, true)).is_none());
        assert!(d.observe(&custom(2, 1, 2, 100, true, true)).is_none());
        // Same account twice: fires on the second.
        assert!(d.observe(&custom(1, 1, 3, 100, true, true)).is_some());
        // Different instrument: separate counter again.
        assert!(d.observe(&custom(1, 2, 4, 100, true, true)).is_none());
        // And the second cross on instrument 2 completes THAT counter.
        assert!(d.observe(&custom(1, 2, 5, 100, true, true)).is_some());
    }

    #[test]
    fn spoofing_total_counts_placements_not_just_cancels() {
        // min_orders=5, ratio 80%: 4 cancels + 1 executed = 80% → alert.
        let mut d = SpoofingDetector::new(5, 8_000);
        let o = custom(1, 1, 1, 100, false, false);
        for _ in 0..3 {
            assert!(d.observe_cancel(&o, 0).is_none());
        }
        // executed_orders participates in the ratio denominator.
        let alert = d.observe_cancel(&o, 1).unwrap();
        // 4 cancels / 5 total = 80% exactly → confidence 80.
        assert_eq!(alert.confidence, 80);
    }

    #[test]
    fn marking_close_zero_or_missing_median_never_alerts() {
        let mut d = MarkingCloseDetector::new(1_000, 1_000, 2);
        // Missing median: no alert (get() → None short-circuit).
        assert!(d.observe(&custom(1, 1, 1_500, 100, false, true)).is_none());
        // Zero median: guarded (`median > &0`).
        d.set_median(1, 0);
        assert!(d.observe(&custom(1, 1, 1_500, 100, false, true)).is_none());
    }

    #[test]
    fn marking_close_window_bounds_are_inclusive() {
        // Window [1_000, 2_000]. Trade exactly at both bounds must count.
        let mut d = MarkingCloseDetector::new(1_000, 1_000, 1);
        d.set_median(1, 10);
        let at_start = custom(1, 1, 1_000, 10, false, true);
        assert!(d.observe(&at_start).is_some());
        let at_end = custom(1, 1, 2_000, 10, false, true);
        assert!(d.observe(&at_end).is_some());
        let after = custom(1, 1, 2_001, 10, false, true);
        assert!(d.observe(&after).is_none());
        // Before the window: no.
        assert!(d.observe(&custom(1, 1, 999, 10, false, true)).is_none());
    }

    #[test]
    fn triage_ties_preserve_open_order() {
        let mut m = CaseManager::new();
        let mk = |p: Pattern| Alert {
            pattern: p,
            instrument_id: 1,
            account_id: 7,
            confidence: 50,
            evidence: vec![],
        };
        // Same severity (3) - stable sort keeps first-opened first.
        m.open(mk(Pattern::Spoofing));
        m.open(mk(Pattern::WashTrade));
        let triage = m.open_by_severity();
        assert_eq!(triage[0].alert.pattern, Pattern::Spoofing);
        assert_eq!(triage[1].alert.pattern, Pattern::WashTrade);
    }

    #[test]
    fn case_ids_are_monotonic() {
        let mut m = CaseManager::new();
        let alert = Alert {
            pattern: Pattern::WashTrade,
            instrument_id: 1,
            account_id: 7,
            confidence: 90,
            evidence: vec![],
        };
        let a = m.open(alert.clone());
        let b = m.open(alert.clone());
        let c = m.open(alert);
        assert!(a < b && b < c);
        assert_eq!(m.cases().len(), 3);
    }
}
