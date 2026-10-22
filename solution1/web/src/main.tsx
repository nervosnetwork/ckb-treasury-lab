import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { ArrowRight, ArrowUpRight, Check, ChevronRight, CircleHelp, Clock3, ExternalLink, LayoutDashboard, LoaderCircle, LogOut, Plus, QrCode, RefreshCw, ShieldCheck, ThumbsDown, ThumbsUp, Vote, Wallet, X } from "lucide-react";
import { ccc } from "@ckb-ccc/shell";
import { ccc as connector } from "@ckb-ccc/connector-react";
import { castVote, withdrawVote } from "../../sdk/src/vote";
import { createProposal, finalizeProposal, passProposal, challengeProposal, recycleProposal } from "../../sdk/src/proposal";
import { createCountingCell } from "../../sdk/src/counting";
import { blake160, requireCellHeader, waitForTransaction } from "../../sdk/src/utils";
import { client, config, fetchSnapshot, network, rpcPort, type ProposalView } from "./chain";
import { connectWallet, WalletSigner } from "./wallet";
import { currentLanguage, translate, type Language, type TranslationKey } from "./i18n";
import "./style.css";

type Page = "overview" | "voter" | "initiator" | "challenger";
type Snapshot = Awaited<ReturnType<typeof fetchSnapshot>>;
type WalletAccount = { signer: ccc.Signer; address: string; lock: ccc.Script };
const walletNetworks = [{ name: network === "testnet" ? "Testnet" : "Develop", client }];
const SHANNONS = 100_000_000n;

function ckb(amount: bigint): string {
  const whole = amount / SHANNONS;
  const decimals = (amount % SHANNONS).toString().padStart(8, "0").replace(/0+$/, "");
  return `${whole.toLocaleString("en-US")}${decimals ? `.${decimals}` : ""}`;
}

function amountFromInput(value: string, language: Language): bigint {
  if (!/^\d+(\.\d{1,8})?$/.test(value)) throw new Error(translate(language, "invalidAmount"));
  const [integer, fraction = ""] = value.split(".");
  const amount = BigInt(integer) * SHANNONS + BigInt(fraction.padEnd(8, "0"));
  if (amount === 0n) throw new Error(translate(language, "zeroAmount"));
  return amount;
}

function short(value: string, start = 12, end = 8) {
  return `${value.slice(0, start)}…${value.slice(-end)}`;
}

function remaining(target: bigint, tip: bigint, language: Language) {
  const blocks = target > tip ? target - tip : 0n;
  return blocks ? translate(language, "blocksAndSeconds", { blocks: String(blocks), seconds: String(blocks * 10n) }) : translate(language, "expired");
}

function estimatedDate(target: bigint, tip: bigint, language: Language) {
  return new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(Date.now() + Number(target - tip) * 10_000));
}

function nonOverlappingCounting(entries: ProposalView["counting"]): ProposalView["counting"] {
  const sorted = [...entries].sort((left, right) => left.data.endHash - right.data.endHash);
  const plans: { amount: bigint; cells: ProposalView["counting"] }[] = [{ amount: 0n, cells: [] }];
  for (const [index, entry] of sorted.entries()) {
    let previous = index;
    while (previous > 0 && sorted[previous - 1].data.endHash >= entry.data.startHash) previous--;
    const candidate = plans[previous];
    const amount = candidate.amount + entry.data.voteAmount;
    plans.push(amount > plans[index].amount ? { amount, cells: [...candidate.cells, entry] } : plans[index]);
  }
  return plans[sorted.length].cells;
}

function App() {
  const [language, setLanguage] = useState<Language>(currentLanguage);
  const t = (key: Parameters<typeof translate>[1], variables?: Record<string, string | number>) => translate(language, key, variables);
  const [page, setPage] = useState<Page>("overview");
  const { open, disconnect: disconnectConnector, signerInfo } = connector.useCcc();
  const [walletSource, setWalletSource] = useState<"ckb-tw" | "ccc">(() => localStorage.getItem("walletSource") === "ckb-tw" ? "ckb-tw" : "ccc");
  const [account, setAccount] = useState<WalletAccount | null>(null);
  const [connecting, setConnecting] = useState(false);
  const walletDialog = useRef<HTMLDialogElement>(null);
  const connectionAttempt = useRef(0);
  const destinationAfterConnect = useRef<Page | undefined>(undefined);
  const connectorSigner = signerInfo?.signer;
  const activeAccount = walletSource === "ccc" && account?.signer !== connectorSigner ? null : account;
  const signer = activeAccount?.signer;
  const lock = activeAccount?.lock;
  const address = activeAccount?.address ?? "";
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  const [description, setDescription] = useState("");
  const [amount, setAmount] = useState("");
  const [recipient, setRecipient] = useState("");
  const [voteChoice, setVoteChoice] = useState<"yes" | "no">("yes");
  const [checkedProposal, setCheckedProposal] = useState<string | null>(null);

  useEffect(() => {
    document.documentElement.lang = language;
    document.title = t("appTitle");
    localStorage.setItem("language", language);
  }, [language]);

  async function refresh(showLoading = true) {
    if (showLoading) setLoading(true);
    try {
      setSnapshot(await fetchSnapshot());
      setNotice((current) => current?.error ? null : current);
    } catch (error) {
      setNotice({ text: t("readFailed", { error: String(error) }), error: true });
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => { void refresh(false); }, 12000);
    return () => clearInterval(timer);
  }, []);

  function connect(destination?: Page) {
    destinationAfterConnect.current = destination;
    walletDialog.current?.showModal();
  }

  function acceptAccount(next: WalletAccount) {
    setAccount(next);
    localStorage.setItem("walletSource", next.signer instanceof WalletSigner ? "ckb-tw" : "ccc");
    setNotice(null);
    if (destinationAfterConnect.current) setPage(destinationAfterConnect.current);
    destinationAfterConnect.current = undefined;
    walletDialog.current?.close();
  }

  async function getCkbTwAccount(): Promise<WalletAccount> {
    const address = await connectWallet();
    if (!address.startsWith("ckt")) throw new Error(t("wrongNetwork", { network: t(network === "testnet" ? "testnet" : "developNet") }));
    const { script } = await ccc.Address.fromString(address, client);
    const secp = await ccc.Script.fromKnownScript(client, ccc.KnownScript.Secp256k1Blake160, script.args);
    if (!script.eq(secp)) throw new Error(t("unsupportedAccount"));
    return { signer: new WalletSigner(client, address, script), address, lock: script };
  }

  useEffect(() => {
    if (walletSource !== "ccc") return;
    const attempt = ++connectionAttempt.current;
    setAccount(null);
    if (!connectorSigner) return;
    void connectorSigner.getRecommendedAddress().then(async (address) => {
      if (connectorSigner.client.url !== client.url) throw new Error(translate(currentLanguage(), "wrongNetwork", { network: translate(currentLanguage(), network === "testnet" ? "testnet" : "developNet") }));
      const parsed = await ccc.Address.fromString(address, client);
      if (attempt === connectionAttempt.current) acceptAccount({ signer: connectorSigner, address, lock: parsed.script });
    }).catch((error) => {
      if (attempt === connectionAttempt.current) setNotice({ text: String(error), error: true });
    });
    return () => { connectionAttempt.current++; };
  }, [connectorSigner, walletSource]);

  useEffect(() => {
    if (walletSource !== "ckb-tw" || account) return;
    const attempt = ++connectionAttempt.current;
    void getCkbTwAccount().then((restored) => {
      if (attempt === connectionAttempt.current) setAccount(restored);
    }).catch((error) => {
      if (attempt === connectionAttempt.current) setNotice({ text: String(error), error: true });
    });
    return () => { connectionAttempt.current++; };
  }, [walletSource, account]);

  async function connectCkbTw() {
    const attempt = ++connectionAttempt.current;
    setConnecting(true);
    try {
      const nextAccount = await getCkbTwAccount();
      if (attempt !== connectionAttempt.current) return;
      disconnectConnector();
      setWalletSource("ckb-tw");
      acceptAccount(nextAccount);
    } catch (error) {
      if (attempt === connectionAttempt.current) setNotice({ text: String(error), error: true });
    } finally {
      if (attempt === connectionAttempt.current) setConnecting(false);
    }
  }

  function connectCcc() {
    connectionAttempt.current++;
    disconnectConnector();
    setAccount(null);
    setWalletSource("ccc");
    localStorage.setItem("walletSource", "ccc");
    walletDialog.current?.close();
    open();
  }

  useEffect(() => {
    if (walletSource !== "ckb-tw" || !(account?.signer instanceof WalletSigner)) return;
    const directSigner = account.signer;
    let cancelled = false;
    const checkAccount = async () => {
      try {
        await directSigner.assertAccount();
      } catch (error) {
        if (!cancelled) {
          setAccount(null);
          setNotice({ text: String(error), error: true });
        }
      }
    };
    window.addEventListener("focus", checkAccount);
    return () => { cancelled = true; window.removeEventListener("focus", checkAccount); };
  }, [account, walletSource]);

  function disconnect() {
    connectionAttempt.current++;
    disconnectConnector();
    setAccount(null);
    localStorage.removeItem("walletSource");
    destinationAfterConnect.current = undefined;
    walletDialog.current?.close();
  }

  function navigate(destination: Page) {
    if (destination !== "overview" && destination !== "challenger" && !signer) {
      setNotice({ text: t("walletRequired"), error: true });
      void connect(destination);
      return;
    }
    setPage(destination);
    setNotice(null);
  }

  async function execute(key: string, label: TranslationKey, action: (active: ccc.Signer) => Promise<string>) {
    if (!signer) return void connect();
    setBusy(key);
    setNotice({ text: t("signing", { action: t(label) }), error: false });
    try {
      if (signer instanceof WalletSigner) {
        try {
          await signer.assertAccount();
        } catch (error) {
          disconnect();
          throw error;
        }
      }
      const result = await action(signer);
      setNotice({ text: t("actionSuccess", { action: t(label), hash: result }), error: false });
      await waitForTransaction(client, result);
      await refresh(false);
    } catch (error) {
      setNotice({ text: t("actionFailed", { action: t(label), error: String(error) }), error: true });
    } finally {
      setBusy("");
    }
  }

  function owned(proposal: ProposalView) {
    return lock?.eq(proposal.owner) ?? false;
  }

  const settings = snapshot?.settings.data;
  const tip = snapshot?.tip ?? 0n;
  const proposals = [...(snapshot?.proposals ?? [])].sort((a, b) => a.origin > b.origin ? -1 : a.origin < b.origin ? 1 : 0);
  const recipientHash = lock ? blake160(lock.toBytes()) : null;
  const active = proposals.filter((p) => p.data.status === 0 && settings && tip < p.origin + settings.voteWindow);
  const mine = proposals.filter((p) => owned(p) || (p.data.status === 1 && p.data.recipientLockHash === recipientHash));
  const finalized = proposals.filter((p) => p.data.status === 1);

  function actionButton(label: TranslationKey, key: string, action: (active: ccc.Signer) => Promise<string>, disabled = false, className = "button-secondary") {
    return <button className={className} disabled={!!busy || disabled} onClick={() => void execute(key, label, action)}>{busy === key ? <LoaderCircle className="spin" size={16} /> : null}{t(label)}</button>;
  }

  function proposalCard(proposal: ProposalView, mode: Page) {
    const { cell, data, origin, votes, counting } = proposal;
    const key = cell.outPoint.txHash;
    const myVote = votes.find((vote) => lock?.eq(vote.cell.cellOutput.lock));
    const direction = mode === "challenger" ? 0 : 1;
    const matching = nonOverlappingCounting(counting.filter((entry) => entry.data.direction === direction && lock?.eq(entry.cell.cellOutput.lock)));
    const counted = matching.reduce((sum, entry) => sum + entry.data.voteAmount, 0n);
    const end = origin + (settings?.voteDuration ?? 0n) + 1n;
    const windowEnd = origin + (settings?.voteWindow ?? 0n);
    const challengeEnd = challengeBlocks[key] === undefined ? null : challengeBlocks[key] + (settings?.challengeTime ?? 0n) + 1n;
    const countdown: { target: bigint | null; label: TranslationKey; estimate: TranslationKey } = data.status === 0
      ? mode === "voter"
        ? { target: windowEnd, label: "votingWindow", estimate: "estimatedExpiry" }
        : { target: end, label: "countingAvailable", estimate: "estimatedCounting" }
      : { target: challengeEnd, label: "challengeDeadline", estimate: "estimatedExpiry" };
    const canCount = mode === "challenger" ? tip > origin + (settings?.voteDuration ?? 0n) : tip >= end;
    const voteOpen = data.status === 0 && tip < windowEnd;
    const hasYesVotes = votes.some((entry) => entry.data.direction === 1);
    const hasYesCounting = counting.some((entry) => entry.data.direction === 1);
    const hasFullRangeCounting = counting.some((entry) => entry.data.direction === direction && lock?.eq(entry.cell.cellOutput.lock) && entry.data.startHash === 0 && entry.data.endHash === 65535);
    const noYesVotes = !hasYesVotes && !hasYesCounting;
    const recycleAt = end + (settings?.challengeTime ?? 0n);
    const threshold = settings?.yesThreshold ?? 0n;
    const checkResult = counted >= threshold && matching.length > 0
      ? t("countedPass", { amount: ckb(counted), next: t(canCount ? "readyToFinalize" : "waitToCount") })
      : noYesVotes
        ? t("noYesVotesMessage", { next: tip >= recycleAt ? t(voteOpen ? "recycleOrWait" : "recycleReady") : t("recycleLater", { time: remaining(recycleAt, tip, language) }) })
        : t(proposal.yes >= threshold ? "votesMeetThreshold" : "votesBelowThreshold", { amount: ckb(proposal.yes), threshold: ckb(threshold) });
    return <article className="proposal" key={`${key}:${cell.outPoint.index}`}>
      <div className="proposal-top"><span className={`tag tag-${data.status}`}>{t((["openStatus", "finalizedStatus", "passedStatus"] as const)[data.status] ?? "unknownStatus")}</span><span className="mono muted">{proposal.id}</span></div>
      <h3>{data.description || t("unnamedProposal")}</h3>
      <div className="proposal-details">
        <div><span>{t("requestedAmount")}</span><strong>{ckb(data.requestedAmount)} <small>CKB</small></strong></div>
        <div><span>{t("recipientHash")}</span><code>{data.recipientLockHash}</code></div>
        <div><span>{t("yesVotes")}</span><strong className="yes">{ckb(proposal.yes)} <small>CKB</small></strong></div>
        <div><span>{t("noVotes")}</span><strong className="no">{ckb(proposal.no)} <small>CKB</small></strong></div>
      </div>
      {data.status !== 2 && <div className="countdown"><Clock3 size={16}/><span>{t(countdown.label)}</span><strong>{countdown.target === null ? t("loadingHeader") : remaining(countdown.target, tip, language)}{countdown.target !== null && tip < countdown.target && <><br/><small>{t(countdown.estimate, { date: estimatedDate(countdown.target, tip, language) })}</small></>}</strong></div>}
      {mode === "voter" && myVote && <div className="vote-record"><Check size={16}/><span>{t("voted", { direction: t(myVote.data.direction === 1 ? "yes" : "no"), amount: ckb(myVote.data.voteAmount) })}</span><span className="muted">{t("blockNumber", { number: voteBlocks[`${myVote.cell.outPoint.txHash}:${myVote.cell.outPoint.index}`] ?? t("loading") })}</span></div>}
      {(mode === "initiator" || mode === "challenger") && matching.length > 0 && <div className="vote-record"><Check size={16}/><span>{t("countingSummary", { count: matching.length, amount: ckb(counted) })}</span></div>}
      <div className="proposal-actions">
        {mode === "voter" && voteOpen && (myVote
          ? actionButton("withdrawVote", `${key}-withdraw`, (s) => withdrawVote(s, config, { voteOutPoint: myVote.cell.outPoint }))
          : <div className="vote-controls"><div className="segmented"><button className={voteChoice === "yes" ? "selected" : ""} onClick={() => setVoteChoice("yes")}><ThumbsUp size={15}/> {t("yes")}</button><button className={voteChoice === "no" ? "selected" : ""} onClick={() => setVoteChoice("no")}><ThumbsDown size={15}/> {t("no")}</button></div>{actionButton("castVote", `${key}-vote`, async (s) => (await castVote(s, config, { proposalOutPoint: cell.outPoint, direction: voteChoice })).txHash, false, "button-primary")}</div>)}
        {mode === "initiator" && data.status === 0 && <>
          <button className="button-secondary" aria-expanded={checkedProposal === proposal.id} onClick={() => setCheckedProposal((current) => current === proposal.id ? null : proposal.id)}>{t("checkPass")}</button>
          {actionButton("countYes", `${key}-count-yes`, async (s) => (await createCountingCell(s, config, { proposalOutPoint: cell.outPoint, direction: "yes", startHash: 0, endHash: 65535 })).txHash, !canCount || !hasYesVotes || counted >= threshold || hasFullRangeCounting)}
          {actionButton("finalize", `${key}-finalize`, (s) => finalizeProposal(s, config, { proposalOutPoint: cell.outPoint, countingOutPoints: matching.map((entry) => entry.cell.outPoint) }), !canCount || counted < (settings?.yesThreshold ?? 0n))}
          {tip >= recycleAt && actionButton("recycle", `${key}-recycle`, (s) => recycleProposal(s, config, { proposalOutPoint: cell.outPoint }))}
        </>}
        {mode === "initiator" && data.status === 1 && data.recipientLockHash === recipientHash && actionButton("pass", `${key}-pass`, (s) => passProposal(s, config, { proposalOutPoint: cell.outPoint }), challengeEnd === null || tip < challengeEnd)}
        {mode === "challenger" && <>
          <button className="button-secondary" onClick={() => setNotice({ text: t(proposal.no >= data.totalYes ? "challengeMet" : "challengeNotMet", { amount: ckb(proposal.no), threshold: ckb(data.totalYes) }), error: false })}>{t("checkChallenge")}</button>
          {actionButton("countNo", `${key}-count-no`, async (s) => (await createCountingCell(s, config, { proposalOutPoint: cell.outPoint, direction: "no", startHash: 0, endHash: 65535 })).txHash, !canCount || counted >= data.totalYes || hasFullRangeCounting || challengeEnd === null || tip >= challengeEnd)}
          {actionButton("challenge", `${key}-challenge`, (s) => challengeProposal(s, config, { proposalOutPoint: cell.outPoint, countingOutPoints: matching.map((entry) => entry.cell.outPoint) }), !matching.length || counted < data.totalYes || challengeEnd === null || tip >= challengeEnd, "button-danger")}
        </>}
      </div>
      {mode === "initiator" && data.status === 0 && checkedProposal === proposal.id && <div className="check-result" role="status">{checkResult}</div>}
    </article>;
  }

  const [voteBlocks, setVoteBlocks] = useState<Record<string, string>>({});
  const [challengeBlocks, setChallengeBlocks] = useState<Record<string, bigint>>({});
  useEffect(() => {
    if (!snapshot) return;
    const allVotes = snapshot.proposals.flatMap((p) => p.votes);
    let cancelled = false;
    void Promise.all([
      Promise.all(allVotes.map(async (vote) => [ `${vote.cell.outPoint.txHash}:${vote.cell.outPoint.index}`, String((await requireCellHeader(client, vote.cell.outPoint)).number)] as const)),
      Promise.all(snapshot.proposals.filter((p) => p.data.status === 1).map(async (p) => [p.cell.outPoint.txHash, (await requireCellHeader(client, p.cell.outPoint)).number] as const)),
    ]).then(([voteEntries, challengeEntries]) => {
      if (!cancelled) { setVoteBlocks(Object.fromEntries(voteEntries)); setChallengeBlocks(Object.fromEntries(challengeEntries)); }
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [snapshot]);

  async function submitProposal(event: React.FormEvent) {
    event.preventDefault();
    await execute("create", "createProposal", async (active) => {
      const recipientAddress = await ccc.Address.fromString(recipient, client);
      const result = await createProposal(active, config, { description: description.trim(), requestedAmount: amountFromInput(amount, language), recipient: recipientAddress.script });
      setDescription(""); setAmount(""); setRecipient("");
      return result.txHash;
    });
  }

  return <div className="app">
    <dialog ref={walletDialog} className="wallet-dialog" aria-labelledby="wallet-dialog-title" onClose={() => { connectionAttempt.current++; setConnecting(false); }}>
      <div className="wallet-dialog-heading"><h2 id="wallet-dialog-title">{t("connectWallet")}</h2><button className="icon-button" title={t("closeNotice")} aria-label={t("closeNotice")} onClick={() => walletDialog.current?.close()}><X size={17}/></button></div>
      {notice?.error && <p className="wallet-error" role="alert">{notice.text}</p>}
      <button className="wallet-option" disabled={!!busy || connecting} onClick={() => void connectCkbTw()}><Wallet size={20}/>ckb-tw{connecting && <LoaderCircle className="spin" size={16}/>}</button>
      <button className="wallet-option" disabled={!!busy || connecting} onClick={connectCcc}><QrCode size={20}/>CCC / Khie<ChevronRight size={17}/></button>
      {signer && <button className="wallet-option" disabled={!!busy} onClick={disconnect}><LogOut size={20}/>{t("disconnectWallet")}</button>}
    </dialog>
    <aside className="sidebar">
      <button className="brand" onClick={() => navigate("overview")}><span className="brand-mark">✳</span><span>CKB <b>Treasury</b><small>{t("brandSubtitle")}</small></span></button>
      <div className="sidebar-label">{t("workspace")}</div>
      <nav aria-label={t("primaryNavigation")}>
        <button className={page === "overview" ? "active" : ""} onClick={() => navigate("overview")}><LayoutDashboard size={18}/> {t("overview")}</button>
        <button className={page === "voter" ? "active" : ""} onClick={() => navigate("voter")}><Vote size={18}/> {t("voter")}</button>
        <button className={page === "initiator" ? "active" : ""} onClick={() => navigate("initiator")}><Plus size={18}/> {t("initiator")}</button>
        <button className={page === "challenger" ? "active" : ""} onClick={() => navigate("challenger")}><ShieldCheck size={18}/> {t("challenger")}</button>
      </nav>
      <div className="sidebar-bottom"><span className="network-dot"/> {t(network === "testnet" ? "testnet" : "developNet")} {rpcPort && <span className="mono">:{rpcPort}</span>}<p>{t(network === "testnet" ? "testNetwork" : "localNetwork")}</p></div>
    </aside>
    <div className="workspace">
      <header className="topbar"><div className="breadcrumb">CKB Treasury <ChevronRight size={14}/> {t(page)}</div><div className="top-actions"><span className="top-network"><span className="network-dot"/> {t(network === "testnet" ? "testnet" : "developNet")}</span><label className="language-control"><span>{t("languageLabel")}</span><select aria-label={t("languageLabel")} value={language} onChange={(event) => { setLanguage(event.target.value as Language); setNotice(null); }}><option value="en-US">{t("english")}</option><option value="zh-CN">{t("chinese")}</option></select></label><button className="icon-button" title={t("refresh")} aria-label={t("refresh")} onClick={() => void refresh()}><RefreshCw size={17} className={loading ? "spin" : ""}/></button><button className="wallet-button" title={address || t("connectWallet")} onClick={() => void connect()}><Wallet size={16}/>{address ? short(address, 9, 5) : t("connectWallet")}</button></div></header>
      <main>
        {notice && <div className={`notice ${notice.error ? "error" : ""}`} role="status"><span>{notice.text}</span><button title={t("closeNotice")} aria-label={t("closeNotice")} onClick={() => setNotice(null)}><X size={16}/></button></div>}
        {page === "overview" && <>
          <div className="page-heading"><div><div className="eyebrow">{t("governanceEyebrow", { network: t(network === "testnet" ? "testnet" : "developNet") })}</div><h1>{t("welcomeTitle")}<span className="accent">.</span></h1><p>{t("welcomeSubtitle")}</p></div><div className="block-indicator"><span className="network-dot"/> {t("chainHeight")} <strong>#{tip.toString()}</strong></div></div>
          <section className="stats" aria-label={t("governanceRules")}><div><span>{t("activeProposals")}</span><strong>{active.length.toString().padStart(2, "0")}</strong><small>{t("activeHint")}</small></div><div><span>{t("challengingProposals")}</span><strong>{finalized.length.toString().padStart(2, "0")}</strong><small>{t("challengingHint")}</small></div><div><span>{t("passedProposals")}</span><strong>{proposals.filter((p) => p.data.status === 2).length.toString().padStart(2, "0")}</strong><small>{t("passedHint")}</small></div></section>
          <div className="section-heading"><div><span className="eyebrow">{t("getStartedEyebrow")}</span><h2>{t("getStarted")}</h2></div></div>
          <div className="entry-grid"><button onClick={() => navigate("initiator")} className="entry entry-dark"><span className="entry-icon"><ArrowUpRight size={23}/></span><span className="entry-title">{t("initiator")}</span><span className="entry-copy">{t("entryInitiator")}</span><span className="entry-link">{t("entryInitiatorLink")} <ArrowRight size={17}/></span></button><button onClick={() => navigate("voter")} className="entry entry-light"><span className="entry-icon"><Vote size={23}/></span><span className="entry-title">{t("voter")}</span><span className="entry-copy">{t("entryVoter")}</span><span className="entry-link">{t("entryVoterLink")} <ArrowRight size={17}/></span></button></div>
          <div className="section-heading config-head"><div><span className="eyebrow">{t("parametersEyebrow")}</span><h2>{t("governanceRules")}</h2></div><span className="muted">{t("fromConfig")}</span></div>
          <section className="config-grid">{settings ? <><div><span>{t("yesThreshold")}</span><strong>{ckb(settings.yesThreshold)} CKB</strong></div><div><span>{t("proposalBond")}</span><strong>{ckb(settings.minimalProposalCapacity)} CKB</strong></div><div><span>{t("minVoteAmount")}</span><strong>{ckb(settings.minimalVoteAmount)} CKB</strong></div><div><span>{t("voteDuration")}</span><strong>{t("blocks", { count: String(settings.voteDuration) })}</strong></div><div><span>{t("voteWindow")}</span><strong>{t("blocks", { count: String(settings.voteWindow) })}</strong></div><div><span>{t("challengeTime")}</span><strong>{t("blocks", { count: String(settings.challengeTime) })}</strong></div><div><span>{t("emergencyHalt")}</span><strong>{t(settings.emergentHalt ? "enabled" : "disabled")}</strong></div><div><span>{t("vetoHash")}</span><code>{settings.vetoLockScriptHash}</code></div></> : <p className="muted">{loading ? t("loadingConfig") : t("configUnavailable")}</p>}</section>
          {settings && <div className="config-technical"><div><span>{t("voteScript", { type: settings.voteHashType })}</span><code>{settings.voteCodeHash}</code></div><div><span>{t("countingScript", { type: settings.countingHashType })}</span><code>{settings.countingCodeHash}</code></div><div><span>{t("alwaysSuccessScript", { type: settings.alwaysSuccessHashType })}</span><code>{settings.alwaysSuccessCodeHash}</code></div></div>}
          <button className="challenge-link" onClick={() => navigate("challenger")}><ShieldCheck size={19}/> {t("viewChallenges")} <ArrowRight size={17}/></button>
        </>}
        {page === "voter" && <><div className="page-heading inner"><div><div className="eyebrow">{t("voterEyebrow")}</div><h1>{t("voter")}<span className="accent">.</span></h1><p>{t("voterSubtitle")}</p></div><div className="block-indicator">{t("openProposalCount", { count: active.length })}</div></div><div className="section-heading"><h2>{t("withinWindow")}</h2><span className="muted">{t("blockEstimate")}</span></div><div className="list">{active.length ? active.map((p) => proposalCard(p, "voter")) : <div className="empty">{t("noOpenProposals")}</div>}</div></>}
        {page === "initiator" && <><div className="page-heading inner"><div><div className="eyebrow">{t("proposalEyebrow")}</div><h1>{t("initiator")}<span className="accent">.</span></h1><p>{t("initiatorSubtitle")}</p></div><div className="block-indicator">{t("myProposalCount", { count: mine.length })}</div></div><section className="create-panel"><div><span className="eyebrow">{t("newProposalEyebrow")}</span><h2>{t("newProposal")}</h2><p>{t("createHint")}</p></div><form onSubmit={(event) => void submitProposal(event)}><label>{t("descriptionLabel")}<textarea required maxLength={500} placeholder={t("descriptionPlaceholder")} value={description} onChange={(event) => setDescription(event.target.value)} /></label><div className="form-row"><label>{t("requestedAmount")} <span>CKB</span><input required inputMode="decimal" placeholder={t("amountPlaceholder")} value={amount} onChange={(event) => setAmount(event.target.value)}/></label><label>{t("recipientAddress")} <span title={t("recipientHelp")}><CircleHelp size={14}/></span><input required value={recipient} onChange={(event) => setRecipient(event.target.value)}/></label></div><button className="button-primary" disabled={!!busy || !signer}>{busy === "create" ? <LoaderCircle className="spin" size={16}/> : <Plus size={17}/>} {t("createProposal")}</button></form></section><div className="section-heading"><h2>{t("myProposals")}</h2><span className="muted">{t("cellCount", { count: mine.length })}</span></div><div className="list">{mine.length ? mine.map((p) => proposalCard(p, "initiator")) : <div className="empty">{t("noMyProposals")}</div>}</div></>}
        {page === "challenger" && <><div className="page-heading inner"><div><div className="eyebrow">{t("challengerEyebrow")}</div><h1>{t("challenger")}<span className="accent">.</span></h1><p>{t("challengerSubtitle")}</p></div><div className="block-indicator">{t("pendingCount", { count: finalized.length })}</div></div><div className="section-heading"><h2>{t("challengeProposals")}</h2><span className="muted">{t("rewardHint")}</span></div><div className="list">{finalized.length ? finalized.map((p) => proposalCard(p, "challenger")) : <div className="empty">{t("noChallenges")}</div>}</div></>}
        <footer><span>CKB Treasury · {t(network === "testnet" ? "testNetwork" : "localNetwork")}</span><span>{t("liveData")} <ExternalLink size={13}/></span></footer>
      </main>
    </div>
  </div>;
}

createRoot(document.getElementById("root")!).render(<connector.Provider name="CKB Treasury" defaultClient={client} clientOptions={walletNetworks}><App /></connector.Provider>);
