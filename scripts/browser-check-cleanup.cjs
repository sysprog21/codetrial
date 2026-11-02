const { drainDiagnosticStreams } = require("./floor-capture.cjs");

// Attempt every owned-resource cleanup; a secondary failure cannot replace
// the run's budget/soak outcome, and the first cleanup failure keeps its cause.
async function cleanupBrowserCheck({
  stopAgent,
  streams,
  browser,
  compilerExplorerMock,
  operationFailed,
}) {
  let failure;
  for (const close of [
    stopAgent,
    () => drainDiagnosticStreams(streams),
    () =>
      compilerExplorerMock &&
      new Promise((resolve, reject) => {
        compilerExplorerMock.server.close((error) =>
          error ? reject(error) : resolve(),
        );
      }),
    () => browser.close(),
  ]) {
    try {
      await close();
    } catch (error) {
      failure ??= error;
    }
  }
  if (failure) {
    if (!operationFailed) throw failure;
    console.error("browser check cleanup also failed after the run failed");
  }
}

module.exports = { cleanupBrowserCheck };
