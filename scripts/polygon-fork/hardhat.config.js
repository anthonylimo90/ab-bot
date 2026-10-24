// A local fork of Polygon mainnet (chain id 137). FORK_URL names the node
// it forks from, which must serve historical state (an archive node).
// FORK_BLOCK pins the fork a little behind the head: load-balanced
// providers route some requests to backends hundreds of blocks behind,
// which do not know a block taken from the head. retrying_relay.py retries
// their other refusals.
// Shanghai rules: Polygon has no blob transactions, and Hardhat's Cancun
// mode fails calls at blocks it mined itself.
module.exports = {
  networks: {
    hardhat: {
      chainId: 137,
      hardfork: "shanghai",
      forking: {
        url: process.env.FORK_URL || "https://polygon.drpc.org",
        blockNumber: process.env.FORK_BLOCK ? Number(process.env.FORK_BLOCK) : undefined,
      },
      chains: { 137: { hardforkHistory: { shanghai: 0 } } },
    },
  },
};
