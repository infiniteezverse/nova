import { useCallback, useEffect, useState } from "react";
import {
  Caps,
  EXPLORER_TX,
  MarketSummary,
  NOVA_CONTRACT_ID,
  Observation,
  connectWallet,
  currentWallet,
  fetchCaps,
  fetchEligible,
  fetchMarket,
  fetchObservation,
  submitCheckpoint,
  submitIntent,
} from "./lib/nova";

type Snapshot = {
  market: MarketSummary;
  eligible: boolean;
  caps: Caps;
  observation: Observation;
};

type LogLine = { at: string; text: string; href?: string };

const ORACLE_STALE_MSG = "Oracle or liquidity check failed. Market is effectively Paused.";

function ts(): string {
  return new Date().toLocaleTimeString();
}

function bpsToPercent(bps: bigint): string {
  return `${(Number(bps) / 100).toFixed(2)}%`;
}

function formatUnits(raw: bigint): string {
  return (Number(raw) / 1e7).toLocaleString(undefined, { maximumFractionDigits: 2 });
}

export default function App() {
  const [marketId, setMarketId] = useState(0);
  const [wallet, setWallet] = useState<string | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<LogLine[]>([]);
  const [updated, setUpdated] = useState<Date | null>(null);

  const append = useCallback((text: string, href?: string) => {
    setLog((prev) => [{ at: ts(), text, href }, ...prev].slice(0, 50));
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [market, eligible, caps, observation] = await Promise.all([
        fetchMarket(marketId),
        fetchEligible(marketId),
        fetchCaps(marketId),
        fetchObservation(marketId),
      ]);
      setSnapshot({ market, eligible, caps, observation });
      setLoadError(null);
      setUpdated(new Date());
    } catch (err) {
      setSnapshot(null);
      setLoadError(err instanceof Error ? err.message : String(err));
    }
  }, [marketId]);

  useEffect(() => {
    refresh();
    currentWallet().then(setWallet).catch(() => setWallet(null));
  }, [refresh]);

  // Poll testnet every 15 seconds so the numbers stay current without a manual refresh.
  useEffect(() => {
    const id = window.setInterval(refresh, 15_000);
    return () => window.clearInterval(id);
  }, [refresh]);

  async function onConnect() {
    try {
      setWallet(await connectWallet());
      append("Wallet connected.");
    } catch (err) {
      append(err instanceof Error ? err.message : String(err));
    }
  }

  async function onIntent() {
    if (!wallet) return;
    setBusy(true);
    try {
      const res = await submitIntent(wallet, marketId);
      append(
        `signal_intent: ${res.status}`,
        `${EXPLORER_TX}${res.hash}`,
      );
      await refresh();
    } catch (err) {
      append(`signal_intent failed: ${err instanceof Error ? err.message : err}`);
    } finally {
      setBusy(false);
    }
  }

  async function onCheckpoint() {
    if (!wallet) return;
    setBusy(true);
    try {
      const res = await submitCheckpoint(wallet, marketId);
      append(
        `checkpoint: ${res.status}`,
        `${EXPLORER_TX}${res.hash}`,
      );
      await refresh();
    } catch (err) {
      append(`checkpoint failed: ${err instanceof Error ? err.message : err}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="terminal">
      <header className="bar">
        <div>
          <strong>NOVA</strong> <span className="dim">State-Bound Asset terminal</span>
          <span className="tag">TESTNET</span>
          <a className="small" href="/snapshot.html" style={{ marginLeft: 12 }}>
            Snapshot and what is not shown
          </a>
        </div>
        {wallet ? (
          <span className="dim mono">{wallet.slice(0, 6)}…{wallet.slice(-4)}</span>
        ) : (
          <button onClick={onConnect}>Connect Freighter</button>
        )}
      </header>

      <section className="panel disclaimer" role="note">
        <strong>Testnet prototype. Not audited. Test tokens with no value.</strong>
        <p style={{ margin: "6px 0 0" }}>
          The oracle is a test feed. One admin key sets its price, so it is not a trustless
          market source. Slashing and the early-withdrawal pause (Rule 09) are open design gaps.
          Full list: the snapshot page, linked above.
        </p>
      </section>

      <section className="panel">
        <div className="row">
          <label>
            Market id{" "}
            <input
              type="number"
              min={0}
              value={marketId}
              onChange={(e) => setMarketId(Math.max(0, Number(e.target.value) || 0))}
            />
          </label>
          <button onClick={refresh} disabled={busy}>Refresh</button>
          <span className="dim mono small">contract {NOVA_CONTRACT_ID}</span>
          <span className="live">
            {updated ? `Live · updated ${updated.toLocaleTimeString()}` : "Connecting to testnet…"}
          </span>
        </div>
      </section>

      {loadError && (
        <section className="panel error">
          <p>Could not read market {marketId} from testnet.</p>
          <p className="mono small">{loadError}</p>
        </section>
      )}

      {snapshot && (
        <>
          <section className="panel">
            <h2>1 · Market</h2>
            <dl className="grid">
              <dt>Symbol</dt>
              <dd className="mono">{snapshot.market.symbol}</dd>
              <dt>State</dt>
              <dd className={`state ${snapshot.market.state.toLowerCase()}`}>
                {snapshot.market.state}
              </dd>
              <dt>Unique signers</dt>
              <dd>{snapshot.market.signers}</dd>
              <dt>Composable shield · is_eligible()</dt>
              <dd className={snapshot.eligible ? "ok" : "bad"}>
                {snapshot.eligible ? "TRUE" : "FALSE"}
              </dd>
            </dl>
          </section>

          <section className="panel">
            <h2>2 · Constitution (live read)</h2>
            <p className="small bad">
              Oracle source: TEST FEED. Admin-set, not a verified SEP-40 source. Not trustless.
            </p>
            <dl className="grid">
              <dt>Live spread</dt>
              <dd>{bpsToPercent(snapshot.observation.spread_bps)}</dd>
              <dt>Live depth (quote side)</dt>
              <dd>{formatUnits(snapshot.observation.depth)}</dd>
              <dt>Oracle price</dt>
              <dd>{formatUnits(snapshot.observation.price)}</dd>
              <dt>Last health reading</dt>
              <dd className={snapshot.observation.health === "Active" ? "ok" : "bad"}>
                {snapshot.observation.health}
              </dd>
            </dl>
            <h3>Capabilities</h3>
            <ul className="caps">
              <li className={snapshot.caps.reference_pricing ? "ok" : "bad"}>
                Reference pricing: {snapshot.caps.reference_pricing ? "ON" : "OFF"}
              </li>
              <li className={snapshot.caps.spot_trading ? "ok" : "bad"}>
                Spot trading: {snapshot.caps.spot_trading ? "ON" : "OFF"}
              </li>
              <li className={snapshot.caps.collateral_eligible ? "ok" : "bad"}>
                Collateral: {snapshot.caps.collateral_eligible ? "ON" : "OFF"}
              </li>
            </ul>
          </section>

          <section className="panel">
            <h2>3 · Permissionless actions</h2>
            {!wallet && <p className="dim">Connect Freighter to sign transactions.</p>}
            <div className="row">
              <button onClick={onIntent} disabled={!wallet || busy || snapshot.market.state !== "Discovery"}>
                Sign intent
              </button>
              <button onClick={onCheckpoint} disabled={!wallet || busy}>
                Run checkpoint
              </button>
            </div>
            <p className="dim small">
              Intent is only accepted in Discovery. A checkpoint writes the current health to the ledger.
            </p>
          </section>

          <section className="panel">
            <h2>4 · Consumer vault guard</h2>
            <p>
              A lending pool calls <code>is_eligible()</code> before a deposit or borrow.
              {snapshot.eligible
                ? " Right now it would pass."
                : " Right now it would revert."}
            </p>
            <p className="dim small">
              The lending vault is deployed on testnet (CCMVZHUOGXOOM6ANLESIDX4S3KRQHKQQOIPIXIXS7UCQJFGOXJQYEV2N).
              This panel shows the eligibility check the vault makes. It does not submit deposits or borrows.
            </p>
            {!snapshot.eligible && <p className="bad small">{ORACLE_STALE_MSG}</p>}
          </section>
        </>
      )}

      <section className="panel">
        <h2>Log</h2>
        {log.length === 0 && <p className="dim small">No actions yet.</p>}
        <ul className="log">
          {log.map((line, i) => (
            <li key={i}>
              <span className="dim mono">{line.at}</span>{" "}
              {line.text}
              {line.href && (
                <>
                  {" "}
                  <a href={line.href} target="_blank" rel="noreferrer">
                    tx
                  </a>
                </>
              )}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
