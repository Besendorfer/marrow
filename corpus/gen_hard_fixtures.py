"""Generate corpus v7's hard fixtures (model-routing step 2) from real
before/after sources, so diffs and labeled line numbers are exact.

Each fixture: files {path: (base_text|None, head_text|None)}, plus labels
located by a unique marker substring in the head text."""
import difflib, json, os, sys, textwrap

OUT = sys.argv[1]  # corpus/fixtures dir


def d(s):
    return textwrap.dedent(s).lstrip("\n")


def unified(base, head):
    b = (base or "").splitlines()
    h = (head or "").splitlines()
    lines = list(difflib.unified_diff(b, h, n=3, lineterm=""))
    return "\n".join(lines[2:]) + "\n"  # drop ---/+++ headers


def line_of(text, marker, nth=0):
    hits = [i + 1 for i, l in enumerate(text.splitlines()) if marker in l]
    assert len(hits) > nth, f"marker not found: {marker!r}"
    return hits[nth]


def write(name, title, body, files, relevant, not_relevant, findings, should_not_flag, verdict, readme, snapshot=None):
    root = os.path.join(OUT, name)
    os.makedirs(root, exist_ok=True)
    pr = {"title": title, "body": body, "files": [{"path": p, "diff": unified(b, h)} for p, (b, h) in files.items()]}
    heads = {p: h for p, (b, h) in files.items()}
    def region(spec):
        path, start_marker, end_marker = spec[0], spec[1], spec[2]
        s = line_of(heads[path], start_marker)
        e = line_of(heads[path], end_marker) if end_marker else s
        return path, s, e
    labels = {"relevant": relevant, "not_relevant": not_relevant, "expected_findings": [], "should_not_flag": []}
    for spec in findings:
        path, s, e = region(spec)
        labels["expected_findings"].append({"path": path, "start_line": s, "end_line": e, "importance": spec[3], "note": spec[4]})
    for spec in should_not_flag:
        path, s, e = region(spec)
        labels["should_not_flag"].append({"path": path, "start_line": s, "end_line": e, "note": spec[3]})
    if not labels["should_not_flag"]:
        del labels["should_not_flag"]
    labels["expected_verdict"] = verdict
    json.dump(pr, open(os.path.join(root, "pr.json"), "w"), indent=2)
    json.dump(labels, open(os.path.join(root, "labels.json"), "w"), indent=2)
    open(os.path.join(root, "README.md"), "w").write(readme)
    if snapshot:
        for side in ("base", "head"):
            for path, text in snapshot[side].items():
                full = os.path.join(root, "repo", side, path)
                os.makedirs(os.path.dirname(full), exist_ok=True)
                open(full, "w").write(text)
    print(name, "→", [(f["path"], f["start_line"], f["end_line"]) for f in labels["expected_findings"]])


# ── 1. buried-authz-ts ────────────────────────────────────────────────────
perm_base = d('''
    import { log } from "../logging/logger";
    import type { Doc, User } from "../types";

    export type Role = "owner" | "admin" | "editor" | "commenter" | "viewer";

    const EDIT_ROLES: Role[] = ["owner", "admin", "editor"];

    export function roleOf(user: User, doc: Doc): Role {
      if (doc.ownerId === user.id) return "owner";
      return doc.members[user.id] ?? "viewer";
    }

    export function canEdit(user: User, doc: Doc): boolean {
      const role = roleOf(user, doc);
      log("perm.check", { user: user.id, doc: doc.id, role });
      return EDIT_ROLES.includes(role);
    }

    export function canComment(user: User, doc: Doc): boolean {
      return roleOf(user, doc) !== "viewer";
    }
''')
perm_head = d('''
    import { logEvent } from "../logging/logger";
    import type { Doc, User } from "../types";

    export type Role = "owner" | "admin" | "editor" | "commenter" | "viewer";

    // Lowest to highest; a role can do everything the roles before it can.
    const RANK: Role[] = ["viewer", "commenter", "editor", "admin", "owner"];

    export function roleOf(user: User, doc: Doc): Role {
      if (doc.ownerId === user.id) return "owner";
      return doc.members[user.id] ?? "viewer";
    }

    function atLeast(role: Role, min: Role): boolean {
      return RANK.indexOf(role) >= RANK.indexOf(min);
    }

    export function canEdit(user: User, doc: Doc): boolean {
      const role = roleOf(user, doc);
      logEvent("perm.check", { user: user.id, doc: doc.id, role });
      return atLeast(role, "commenter");
    }

    export function canComment(user: User, doc: Doc): boolean {
      return atLeast(roleOf(user, doc), "commenter");
    }
''')
logger_base = d('''
    export function log(event: string, fields: Record<string, unknown> = {}): void {
      console.log(JSON.stringify({ event, ...fields, at: new Date().toISOString() }));
    }
''')
logger_head = logger_base.replace("export function log(", "export function logEvent(")


def caller(name, fn, body_lines):
    base = f'import {{ log }} from "../logging/logger";\n\nexport async function {fn}(id: string) {{\n' + "".join(f"  {l}\n" for l in body_lines) + "}\n"
    head = base.replace("{ log }", "{ logEvent }").replace("log(", "logEvent(").replace("logEvent }", "logEvent }")
    return base, head


callers = {}
for path, fn, lines in [
    ("src/billing/invoices.ts", "sendInvoice", ['log("invoice.send", { id });', "await mailer.send(id);"]),
    ("src/billing/refunds.ts", "issueRefund", ['log("refund.issue", { id });', "await payments.refund(id);"]),
    ("src/docs/share.ts", "shareDoc", ['log("doc.share", { id });', "await links.create(id);"]),
    ("src/docs/export.ts", "exportDoc", ['log("doc.export", { id });', "await exporter.run(id);"]),
    ("src/users/invite.ts", "inviteUser", ['log("user.invite", { id });', "await invites.send(id);"]),
    ("src/users/deactivate.ts", "deactivateUser", ['log("user.deactivate", { id });', "await users.deactivate(id);"]),
    ("src/search/reindex.ts", "reindexDoc", ['log("search.reindex", { id });', "await search.reindex(id);"]),
]:
    b, h = caller(path, fn, lines)
    b = b.replace("../logging/logger", "../logging/logger")
    callers[path] = (b, h)

files1 = {"src/logging/logger.ts": (logger_base, logger_head), "src/auth/permissions.ts": (perm_base, perm_head)}
files1.update(callers)
files1["CHANGELOG.md"] = ("# Changelog\n\n- 2.3.0: faster exports.\n", "# Changelog\n\n- Internal: rename log() to logEvent().\n- 2.3.0: faster exports.\n")
write(
    "buried-authz-ts",
    "refactor: rename log() to logEvent() and tidy permission helpers",
    "Mechanical rename of the logging helper across services (`log` → `logEvent`, so it stops shadowing debug helpers), plus a small tidy of the permission helpers so they share one role ordering. No behavior change intended.",
    files1,
    relevant=["src/logging/logger.ts", "src/auth/permissions.ts"] + list(callers),
    not_relevant=["CHANGELOG.md"],
    findings=[("src/auth/permissions.ts", 'return atLeast(role, "commenter");', None, "important",
               "canEdit now allows anyone at least 'commenter'; before, only owner/admin/editor could edit. Commenters gain edit rights. Buried in a 10-file mechanical rename whose body says no behavior change.")],
    should_not_flag=[("src/billing/invoices.ts", 'logEvent("invoice.send"', None, "Mechanical logger rename — the stated purpose.")],
    verdict="fix_first",
    readme=d('''
        # buried-authz-ts

        Hard fixture (corpus v7, model-routing step 2). A ten-file "mechanical
        rename" PR whose permission "tidy" introduces a role ranking and makes
        `canEdit` require at least `commenter` instead of `editor`: commenters can
        now edit. The body claims no behavior change. Tests whether a review keeps
        its attention through noise. `canComment` is correct.
    '''),
)

# ── 2. parallel-withdrawals-ts ────────────────────────────────────────────
wd_base = d('''
    import { ledger } from "./ledger";
    import type { Account, Result, Withdrawal } from "./types";

    /** Applies a batch of withdrawals to one account, in order. */
    export async function processWithdrawals(account: Account, requests: Withdrawal[]): Promise<Result[]> {
      const results: Result[] = [];
      for (const req of requests) {
        const balance = await ledger.balance(account.id);
        if (balance < req.amount) {
          results.push({ id: req.id, ok: false, reason: "insufficient funds" });
          continue;
        }
        await ledger.debit(account.id, req.amount);
        results.push({ id: req.id, ok: true });
      }
      return results;
    }
''')
wd_head = d('''
    import { ledger } from "./ledger";
    import type { Account, Result, Withdrawal } from "./types";

    /** Applies a batch of withdrawals to one account. Results keep request order. */
    export async function processWithdrawals(account: Account, requests: Withdrawal[]): Promise<Result[]> {
      return Promise.all(
        requests.map(async (req): Promise<Result> => {
          const balance = await ledger.balance(account.id);
          if (balance < req.amount) {
            return { id: req.id, ok: false, reason: "insufficient funds" };
          }
          await ledger.debit(account.id, req.amount);
          return { id: req.id, ok: true };
        }),
      );
    }
''')
test_base = d('''
    import { processWithdrawals } from "../../src/ledger/withdraw";

    test("a withdrawal larger than the balance is refused", async () => {
      const results = await processWithdrawals(account(50), [{ id: "w1", amount: 80 }]);
      expect(results).toEqual([{ id: "w1", ok: false, reason: "insufficient funds" }]);
    });
''')
test_head = test_base + d('''

    test("results keep request order", async () => {
      const results = await processWithdrawals(account(500), [{ id: "a", amount: 10 }, { id: "b", amount: 20 }]);
      expect(results.map((r) => r.id)).toEqual(["a", "b"]);
    });
''')
write(
    "parallel-withdrawals-ts",
    "perf(ledger): process a batch of withdrawals concurrently",
    "Withdrawals in a batch were applied one at a time, so a large batch took several seconds. Run them concurrently with Promise.all; results still come back in request order (new test).",
    {"src/ledger/withdraw.ts": (wd_base, wd_head), "tests/ledger/withdraw.test.ts": (test_base, test_head)},
    relevant=["src/ledger/withdraw.ts"],
    not_relevant=["tests/ledger/withdraw.test.ts"],
    findings=[("src/ledger/withdraw.ts", "const balance = await ledger.balance(account.id);", "await ledger.debit(account.id, req.amount);", "important",
               "Concurrent read-then-debit: every request reads the balance before any debit lands, so a batch whose total exceeds the balance is fully approved and the account overdraws. Sequential order was load-bearing.")],
    should_not_flag=[],
    verdict="fix_first",
    readme=d('''
        # parallel-withdrawals-ts

        Hard fixture (corpus v7). A performance change turns a sequential
        check-then-debit loop into Promise.all. Each withdrawal checks the balance
        before any debit lands, so a batch can overdraw the account. The new test
        only checks result order. Tests reasoning about concurrency, not syntax.
    '''),
)

# ── 3. broad-retry-py (outside-diff consequence) ──────────────────────────
retry_base = d('''
    import time
    from functools import wraps

    from svc.errors import TransientError


    def retry(times=3, delay=0.5):
        """Retry a call that failed with a transient error."""
        def deco(fn):
            @wraps(fn)
            def wrapper(*args, **kwargs):
                for attempt in range(times):
                    try:
                        return fn(*args, **kwargs)
                    except TransientError:
                        if attempt == times - 1:
                            raise
                        time.sleep(delay * (attempt + 1))
            return wrapper
        return deco
''')
retry_head = retry_base.replace(
    '    """Retry a call that failed with a transient error."""',
    '    """Retry a call that failed. Transient failures surface as many exception\n    types (timeouts, 502s raised as ValueError by the JSON parser), so retry\n    on any exception."""',
).replace("except TransientError:", "except Exception:").replace("\nfrom svc.errors import TransientError\n", "\n")
orders = d('''
    from svc import db, payments
    from svc.models import Order
    from svc.retry import retry


    @retry(times=3)
    def create_order(cart):
        resp = payments.charge(cart.total, cart.card)  # charges the card
        order = Order(id=resp.json()["order_id"])
        db.save(order)
        return order
''')
payments = d('''
    import requests

    API = "https://payments.internal/v1"


    def charge(amount, card):
        """POST a charge. Not idempotent: each call charges the card."""
        return requests.post(f"{API}/charges", json={"amount": amount, "card": card}, timeout=10)
''')
write(
    "broad-retry-py",
    "fix(retry): retry on any exception, not just TransientError",
    "Transient failures show up as a variety of exception types (timeouts, 502s that surface as ValueError from the JSON parser), so narrowing retries to TransientError missed real transient cases. Retry on any exception instead.",
    {"svc/retry.py": (retry_base, retry_head)},
    relevant=["svc/retry.py"],
    not_relevant=[],
    findings=[("svc/retry.py", "except Exception:", None, "important",
               "svc/orders.py (unchanged, outside the diff) wraps create_order in @retry; it charges the card and then parses the response. Any exception after the charge (a KeyError on a missing order_id, a DB error) now retries and charges the card again, up to 3 times. Only knowable by reading callers.")],
    should_not_flag=[],
    verdict="fix_first",
    readme=d('''
        # broad-retry-py

        Hard fixture (corpus v7; `repo/` snapshot). The retry decorator widens from
        TransientError to any exception. The damage is outside the diff:
        `svc/orders.py` retries `create_order`, which charges a card (a
        non-idempotent POST in `svc/payments.py`) before code that can raise. A
        retry after a successful charge charges again. Needs reading two unchanged
        files.
    '''),
    snapshot={
        "base": {"svc/retry.py": retry_base, "svc/orders.py": orders, "svc/payments.py": payments},
        "head": {"svc/retry.py": retry_head, "svc/orders.py": orders, "svc/payments.py": payments},
    },
)

# ── 4. zero-means-unlimited-rs (two hops outside the diff) ────────────────
cfg_base = d('''
    /// Connection pool settings, loaded from `pool.toml`.
    #[derive(Debug, Clone)]
    pub struct PoolConfig {
        pub max_connections: usize,
        pub idle_timeout_secs: u64,
    }

    impl Default for PoolConfig {
        fn default() -> Self {
            PoolConfig {
                max_connections: 10,
                idle_timeout_secs: 300,
            }
        }
    }
''')
cfg_head = cfg_base.replace(
    "    pub max_connections: usize,",
    "    /// Upper bound on open connections; 0 means no limit.\n    pub max_connections: usize,",
).replace("max_connections: 10,", "max_connections: 0,")
builder = d('''
    use crate::config::PoolConfig;
    use crate::pool::Pool;

    pub fn pool_from_config(cfg: &PoolConfig) -> Pool {
        Pool::new(cfg.max_connections, cfg.idle_timeout_secs)
    }
''')
pool = d('''
    pub struct Pool {
        max: usize,
        in_use: usize,
        idle_timeout_secs: u64,
    }

    #[derive(Debug)]
    pub enum PoolError {
        Exhausted,
    }

    impl Pool {
        pub fn new(max: usize, idle_timeout_secs: u64) -> Pool {
            Pool { max, in_use: 0, idle_timeout_secs }
        }

        pub fn acquire(&mut self) -> Result<Conn, PoolError> {
            if self.in_use >= self.max {
                return Err(PoolError::Exhausted);
            }
            self.in_use += 1;
            Ok(Conn::open())
        }
    }
''')
write(
    "zero-means-unlimited-rs",
    "feat(config): don't cap pool connections by default",
    "Bursty workloads were getting throttled by the default cap of 10 connections. Default `max_connections` to 0, meaning no limit; deployments that want a cap can still set one in pool.toml.",
    {"src/config.rs": (cfg_base, cfg_head)},
    relevant=["src/config.rs"],
    not_relevant=[],
    findings=[("src/config.rs", "max_connections: 0,", None, "important",
               "Nothing implements '0 = no limit': src/pool/builder.rs passes max_connections straight to Pool::new, and src/pool/mod.rs acquire() returns Exhausted when in_use >= max, so with 0 every acquire fails and every request errors. Two hops outside the diff.")],
    should_not_flag=[],
    verdict="fix_first",
    readme=d('''
        # zero-means-unlimited-rs

        Hard fixture (corpus v7; `repo/` snapshot). The default connection cap
        becomes 0, which the PR says means "no limit". Nothing implements that:
        `pool/builder.rs` passes the value to `Pool::new`, and `pool/mod.rs`
        refuses to acquire when `in_use >= max`, so every acquire fails. Needs
        following the value through two unchanged files.
    '''),
    snapshot={
        "base": {"src/config.rs": cfg_base, "src/pool/builder.rs": builder, "src/pool/mod.rs": pool},
        "head": {"src/config.rs": cfg_head, "src/pool/builder.rs": builder, "src/pool/mod.rs": pool},
    },
)

# ── 5. looks-scary-correct-ts (the right answer is ship) ─────────────────
inv_base = d('''
    import type { Invoice } from "./types";

    export function renderInvoice(id: string, store: Map<string, Invoice>): string {
      const inv = store.get(id);
      if (!inv) return "<p>Invoice not found</p>";
      return `<section>${formatInvoice(inv)}</section>`;
    }

    export function formatInvoice(inv: Invoice | null): string {
      if (!inv) return "";
      let out = `<h2>Invoice ${inv.number}</h2>`;
      for (let i = 0; i < inv.lines.length; i++) {
        const line = inv.lines[i];
        out += `<p>${line.label}: ${line.amount.toFixed(2)}</p>`;
      }
      return out;
    }
''')
inv_head = d('''
    import type { Invoice } from "./types";

    export function renderInvoice(id: string, store: Map<string, Invoice>): string {
      const inv = store.get(id);
      if (!inv) return "<p>Invoice not found</p>";
      return `<section>${formatInvoice(inv)}</section>`;
    }

    // Only renderInvoice calls this, after its own not-found check, so the
    // invoice is never null here; the type now says so.
    export function formatInvoice(inv: Invoice): string {
      let out = `<h2>Invoice ${inv.number}</h2>`;
      for (const line of inv.lines) {
        out += `<p>${line.label}: ${line.amount.toFixed(2)}</p>`;
      }
      return out;
    }
''')
write(
    "looks-scary-correct-ts",
    "refactor(invoices): drop the unreachable null path in formatInvoice",
    "formatInvoice is only called from renderInvoice, which already returns early when the invoice is missing. Drop the redundant null branch and tighten the parameter type so the compiler enforces it; switch the index loop to for-of while here.",
    {"src/invoices/render.ts": (inv_base, inv_head)},
    relevant=["src/invoices/render.ts"],
    not_relevant=[],
    findings=[],
    should_not_flag=[("src/invoices/render.ts", "export function formatInvoice(inv: Invoice): string {", 'out += `<p>${line.label}', "Removing the null path is safe: the only caller checks first and the type now forbids null. Flagging it is noise."),],
    verdict="ship",
    readme=d('''
        # looks-scary-correct-ts

        Hard fixture (corpus v7). A change that looks risky and isn't: a null
        check is removed, but the only caller (in the same diff) returns early on a
        missing invoice and the parameter type now excludes null. The right review
        is "ship" with nothing flagged. Tests false alarms, where a weaker model
        might over-warn.
    '''),
)
