// Drives scripts/run-budget-guard.cjs the way a worker would: join the run from
// the environment, ask for each action named on the command line, and print
// what was granted and what was refused. It dispatches nothing.
"use strict";

const path = require("path");
const { BudgetDenied, loadRunBudget } = require(
  path.join(process.env.IRB_ROOT, "scripts", "run-budget-guard.cjs"),
);

(async () => {
  let budget;
  try {
    budget = await loadRunBudget(process.env);
  } catch (error) {
    if (!(error instanceof BudgetDenied)) throw error;
    console.log(JSON.stringify({ refused: error.message }));
    process.exit(2);
  }
  if (budget === null) {
    console.log(JSON.stringify({ budget: null }));
    return;
  }
  const out = { granted: [], denied: {}, dispatched: [] };
  const ask = async (id) => {
    try {
      await budget.guarded(id, () => out.dispatched.push(id));
      out.granted.push(id);
    } catch (error) {
      if (!(error instanceof BudgetDenied)) throw error;
      out.denied[id] = error.message;
    }
  };
  const ids = process.argv.slice(2);
  // Overlapping asks are what a page doing two things at once would send.
  if (process.env.DRIVE_CONCURRENT) await Promise.all(ids.map(ask));
  else for (const id of ids) await ask(id);
  console.log(JSON.stringify(out));
})();
