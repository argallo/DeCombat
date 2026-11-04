// DeCombat beta signup form: posts to the lobby's /beta/signup (CORS allows https://decombat.com only).
// Without JavaScript the form still posts and the lobby answers with a plain page. Provenance: ORIGINAL
const BETA_ENDPOINT = "https://beta.decombat.com/beta/signup";

const form = document.getElementById("beta-form");
if (form) {
  const status = document.getElementById("beta-status");
  const submit = document.getElementById("beta-submit");
  const say = (text, ok) => {
    status.textContent = text;
    status.className = "beta-status fine " + (ok ? "ok" : "err");
  };
  form.setAttribute("action", BETA_ENDPOINT);
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const email = document.getElementById("beta-email").value.trim();
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
      say("Enter a valid e-mail address.", false);
      return;
    }
    if (!document.getElementById("beta-consent").checked) {
      say("Tick the box so we may store your address for the invite.", false);
      return;
    }
    submit.disabled = true;
    say("Sending...", true);
    try {
      const r = await fetch(BETA_ENDPOINT, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({
          email,
          platform: document.getElementById("beta-platform").value,
          consent: true,
        }),
      });
      const j = await r.json().catch(() => ({}));
      if (r.ok && j.ok) {
        say(j.message || "Almost done: check your inbox (and the spam or junk folder) for the confirmation link.", true);
        form.reset();
      } else {
        say(j.message || `That did not work (HTTP ${r.status}). Please try again later.`, false);
      }
    } catch {
      say("Could not reach the signup server. Please try again later.", false);
    } finally {
      submit.disabled = false;
    }
  });
}
