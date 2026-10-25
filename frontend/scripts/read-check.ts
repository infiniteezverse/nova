import { fetchMarket, fetchState, fetchEligible, fetchCaps, fetchObservation } from "../src/lib/nova";

const id = 0;
const [market, state, eligible, caps, obs] = await Promise.all([
  fetchMarket(id),
  fetchState(id),
  fetchEligible(id),
  fetchCaps(id),
  fetchObservation(id),
]);
console.log(JSON.stringify({ market, state, eligible, caps, obs }, (_k, v) => (typeof v === "bigint" ? v.toString() : v), 2));
