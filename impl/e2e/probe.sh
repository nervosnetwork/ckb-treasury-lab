#!/usr/sbin/dtrace -s

#pragma D option quiet

dtrace:::BEGIN
{
	printf("Tracing ckb proposal verification via USDT; Ctrl-C to stop.\n\n");
}

/* ── verify() ────────────────────────────────────────────────── */

proposal_probe$target:::verify_entry
{
	self->verify_start = timestamp;
}

proposal_probe$target:::verify_exit
/self->verify_start/
{
	this->elapsed = timestamp - self->verify_start;
	this->n = ++verify_calls[tid];
	@verify_count = count();
	@verify_total_ns = sum(this->elapsed);
	@verify_avg_ns = avg(this->elapsed);
	@verify_min_ns = min(this->elapsed);
	@verify_max_ns = max(this->elapsed);
	@verify_quant_ns = quantize(this->elapsed);
	printf("[verify ] TID %-6d  call# %-4d  %-12d ns\n", tid, this->n, this->elapsed);
	self->verify_start = 0;
}

/* ── BlockProvider calls (combined) ──────────────────────────── */

proposal_probe$target:::block_provider_entry
{
	self->bp_start = timestamp;
}

proposal_probe$target:::block_provider_exit
/self->bp_start/
{
	this->elapsed = timestamp - self->bp_start;
	this->n = ++bp_calls[tid];
	@bp_count = count();
	@bp_total_ns = sum(this->elapsed);
	@bp_avg_ns = avg(this->elapsed);
	@bp_min_ns = min(this->elapsed);
	@bp_max_ns = max(this->elapsed);
	@bp_quant_ns = quantize(this->elapsed);
	self->bp_start = 0;
}

END
{
	printf("\n");
	printf("╔════════════════════════════════════════════════════╗\n");
	printf("║                verify()  Summary                   ║\n");
	printf("╚════════════════════════════════════════════════════╝\n");
	printa("  Total count:  %@d\n", @verify_count);
	printa("  Total time:   %@d ns\n", @verify_total_ns);
	printa("  Average:      %@d ns\n", @verify_avg_ns);
	printa("  Min:          %@d ns\n", @verify_min_ns);
	printa("  Max:          %@d ns\n", @verify_max_ns);
	printf("\n  Latency distribution (ns):\n");
	printa(@verify_quant_ns);

	printf("\n");
	printf("╔════════════════════════════════════════════════════╗\n");
	printf("║          BlockProvider calls  Summary              ║\n");
	printf("╚════════════════════════════════════════════════════╝\n");
	printa("  Total count:  %@d\n", @bp_count);
	printa("  Total time:   %@d ns\n", @bp_total_ns);
	printa("  Average:      %@d ns\n", @bp_avg_ns);
	printa("  Min:          %@d ns\n", @bp_min_ns);
	printa("  Max:          %@d ns\n", @bp_max_ns);
	printf("\n  Latency distribution (ns):\n");
	printa(@bp_quant_ns);
}
