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
import {
  BASE_TOKEN_ID,
  BOND_TOKEN_ID,
  DEMO,
  DemoRun,
  balanceOf,
  createDemoMarket,
  degradeSwapAmount,
  loadRuns,
  reclaimBond,
  saveRun,
  swapToDegrade,
} from "./lib/demo";

const ORACLE_MAX_AGE_SECS = DEMO.maxOracleAgeSecs;

/** What a non-eligible market means, worded for the state it is actually in. */
function stateMessage(state: string): string {
  switch (state) {
    case "Degraded":
      return "Market is Degraded. Spot trading and new deposits or borrows are halted. Liquidations can still settle.";
    case "Paused":
      return "Market is Paused. The oracle is stale or missing, so the market is halted for everyone.";
    case "Discovery":
    case "Ready":
      return "Market is not active yet. Intents move it forward, and a checkpoint activates it once the pool is healthy.";
    default:
      return `Market is ${state}. New deposits and borrows are refused.`;
  }
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

type Snapshot = {
  market: MarketSummary;
  eligible: boolean;
  caps: Caps;
  observation: Observation;
};

type LogLine = { at: string; text: string; href?: string };


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
  const [runs, setRuns] = useState<DemoRun[]>(() => loadRuns());
  const [balances, setBalances] = useState<{ tlith: bigint; tbond: bigint } | null>(null);

  useEffect(() => {
    if (!wallet) {
      setBalances(null);
      return;
    }
    Promise.all([balanceOf(BASE_TOKEN_ID, wallet), balanceOf(BOND_TOKEN_ID, wallet)])
      .then(([tlith, tbond]) => setBalances({ tlith, tbond }))
      .catch(() => setBalances(null));
  }, [wallet, busy]);

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

  async function onCreateRun() {
    if (!wallet) return;
    setBusy(true);
    try {
      const { result, marketId: created } = await createDemoMarket(wallet);
      saveRun({ marketId: created, createdAt: new Date().toISOString(), wallet });
      setRuns(loadRuns());
      setMarketId(created);
      append(`Run started: market ${created} created`, `${EXPLORER_TX}${result.hash}`);
      await refresh();
    } catch (err) {
      append(`Create run failed: ${errMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  async function onSwap() {
    if (!wallet) return;
    setBusy(true);
    try {
      const amount = await degradeSwapAmount();
      append(`Swapping ${(Number(amount) / 1e7).toLocaleString()} TLITH into the pool`);
      const res = await swapToDegrade(wallet, amount);
      append(`Swap into the pool: ${res.status}`, `${EXPLORER_TX}${res.hash}`);
      await refresh();
    } catch (err) {
      append(`Swap failed: ${errMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  async function onReclaim() {
    if (!wallet) return;
    setBusy(true);
    try {
      const res = await reclaimBond(wallet, marketId);
      append(`Reclaim bond: ${res.status}`, `${EXPLORER_TX}${res.hash}`);
      await refresh();
    } catch (err) {
      append(`Reclaim failed: ${errMessage(err)}`);
    } finally {
      setBusy(false);
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

      <section className="panel">
        <h2>1 · Create Market</h2>
        {!wallet && (
          <p className="dim small">
            Connect Freighter on Testnet to run the demo yourself. Reads work without a wallet.
          </p>
        )}
        {wallet && (
          <p className="small">
            Wallet <span className="mono">{wallet.slice(0, 6)}…{wallet.slice(-4)}</span>
            {balances ? (
              <>
                {" "}· TLITH {(Number(balances.tlith) / 1e7).toLocaleString()} · TBOND{" "}
                {(Number(balances.tbond) / 1e7).toLocaleString()}
              </>
            ) : (
              " · balances unavailable"
            )}
          </p>
        )}
        {wallet && balances && balances.tbond < DEMO.bondAmount && (
          <p className="warn small">
            Creating a run needs 1,000 TBOND in this wallet. Ask the operator to fund it.
          </p>
        )}
        {snapshot && snapshot.observation.oracle_timestamp > 0n &&
          Date.now() / 1000 - Number(snapshot.observation.oracle_timestamp) > ORACLE_MAX_AGE_SECS && (
            <p className="bad small">
              The oracle price is stale, so the market is paused for everyone. The operator must
              refresh it with demo/refresh-oracle.sh before a run can continue.
            </p>
          )}
        <div className="row" style={{ marginTop: 8 }}>
          <button onClick={onCreateRun} disabled={!wallet || busy}>
            Create Market
          </button>
          <button onClick={onIntent} disabled={!wallet || busy || snapshot?.market.state !== "Discovery"}>
            Sign intent
          </button>
          <button onClick={onCheckpoint} disabled={!wallet || busy}>
            Checkpoint
          </button>
          <button onClick={onSwap} disabled={!wallet || busy}>
            Swap into pool
          </button>
          <button onClick={onReclaim} disabled={!wallet || busy}>
            Reclaim bond
          </button>
        </div>
        <p className="dim small" style={{ marginTop: 8 }}>
          Sign intent, checkpoint and swap act on the market in the Market id field. Reclaim bond needs the one-hour lock to
          have passed. Each run uses a new market, so a run can be tried again without a reset of the
          contracts.
        </p>
        {runs.length > 0 && (
          <div className="row small" style={{ marginTop: 8 }}>
            <span className="dim">Your Markets:</span>
            {runs.map((r) => (
              <button key={r.marketId} onClick={() => setMarketId(r.marketId)} disabled={busy}>
                market {r.marketId}
              </button>
            ))}
          </div>
        )}
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
            <h2>2 · Permissionless actions</h2>
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
            <h2>3 · Pool and oracle (shared inputs)</h2>
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
            <h2>4 · Market</h2>
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
            <h2>5 · Consumer vault guard</h2>
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
            {!snapshot.eligible && <p className="bad small">{stateMessage(snapshot.market.state)}</p>}
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
