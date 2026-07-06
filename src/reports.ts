import { invoke } from "@tauri-apps/api/core";

// ---------------------------------------------------------------------------
// Reports view: on-demand analytics computed entirely in the frontend from the
// existing `get_all_day_records` command (no backend changes). Each DayRecord
// carries a total plus laps with real unix start/end timestamps, which is enough
// for time aggregates (Week / Month / Year) AND time-of-day rhythm charts.
//
// Charts are hand-rolled inline SVG / CSS — no chart library — to keep the app's
// zero-frontend-deps, local-first footprint. Palette follows the dataviz skill:
// a single blue series hue, sequential blue ramp for heatmaps, text in ink
// tokens (never the data color).
// ---------------------------------------------------------------------------

interface Lap {
  start_time: number;
  end_time?: number;
  duration?: number;
}

interface DayRecord {
  date: string;
  total_duration: number;
  laps: Lap[];
  is_active: boolean;
}

type Mode = "week" | "month" | "year";

const WEEKDAY_LABELS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_LABELS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

export class ReportsView {
  private root: HTMLElement;
  private mode: Mode = "week";
  private anchor = new Date(); // any date inside the currently-viewed period
  private records: DayRecord[] = [];
  private byDate = new Map<string, DayRecord>();
  private tooltip: HTMLDivElement | null = null;

  constructor(root: HTMLElement) {
    this.root = root;
  }

  // Called each time the Reports tab is opened so numbers stay fresh.
  async open(): Promise<void> {
    await this.loadData();
    this.render();
  }

  private async loadData(): Promise<void> {
    try {
      this.records = await invoke<DayRecord[]>("get_all_day_records");
    } catch (e) {
      console.error("Failed to load day records for reports:", e);
      this.records = [];
    }
    this.byDate = new Map(this.records.map((r) => [r.date, r]));
  }

  // ---- date helpers (all LOCAL, matching how day_key is stored) -------------

  private static keyOf(d: Date): string {
    const y = d.getFullYear();
    const m = (d.getMonth() + 1).toString().padStart(2, "0");
    const day = d.getDate().toString().padStart(2, "0");
    return `${y}-${m}-${day}`;
  }

  private static startOfWeek(d: Date): Date {
    const s = new Date(d.getFullYear(), d.getMonth(), d.getDate());
    const dow = (s.getDay() + 6) % 7; // Monday = 0
    s.setDate(s.getDate() - dow);
    return s;
  }

  // Inclusive [start, end] of the currently-anchored period.
  private periodRange(): { start: Date; end: Date } {
    const a = this.anchor;
    if (this.mode === "week") {
      const start = ReportsView.startOfWeek(a);
      const end = new Date(start);
      end.setDate(start.getDate() + 6);
      return { start, end };
    }
    if (this.mode === "month") {
      return {
        start: new Date(a.getFullYear(), a.getMonth(), 1),
        end: new Date(a.getFullYear(), a.getMonth() + 1, 0),
      };
    }
    return {
      start: new Date(a.getFullYear(), 0, 1),
      end: new Date(a.getFullYear(), 11, 31),
    };
  }

  // Every calendar day in the period with its tracked total (0 if untracked).
  private daysInPeriod(): { date: Date; key: string; total: number }[] {
    const { start, end } = this.periodRange();
    const out: { date: Date; key: string; total: number }[] = [];
    const cur = new Date(start);
    while (cur <= end) {
      const key = ReportsView.keyOf(cur);
      out.push({
        date: new Date(cur),
        key,
        total: this.byDate.get(key)?.total_duration ?? 0,
      });
      cur.setDate(cur.getDate() + 1);
    }
    return out;
  }

  private shift(dir: number): void {
    const a = this.anchor;
    if (this.mode === "week") a.setDate(a.getDate() + dir * 7);
    else if (this.mode === "month") a.setMonth(a.getMonth() + dir);
    else a.setFullYear(a.getFullYear() + dir);
    this.anchor = new Date(a);
    this.render();
  }

  // True when the anchored period contains today (so we disable "next").
  private isCurrentPeriod(): boolean {
    const { start, end } = this.periodRange();
    const now = new Date();
    return now >= start && now <= new Date(end.getFullYear(), end.getMonth(), end.getDate(), 23, 59, 59);
  }

  private periodLabel(): string {
    const { start, end } = this.periodRange();
    if (this.mode === "week") {
      const opts: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
      return `${start.toLocaleDateString(undefined, opts)} – ${end.toLocaleDateString(undefined, opts)}, ${end.getFullYear()}`;
    }
    if (this.mode === "month") {
      return this.anchor.toLocaleDateString(undefined, { month: "long", year: "numeric" });
    }
    return `${this.anchor.getFullYear()}`;
  }

  // Current consecutive-day streak ending today (global, not period-bound).
  private currentStreak(): number {
    let streak = 0;
    const cur = new Date();
    // If today isn't tracked yet, start counting from yesterday so an in-progress
    // day doesn't reset the streak to zero.
    if ((this.byDate.get(ReportsView.keyOf(cur))?.total_duration ?? 0) <= 0) {
      cur.setDate(cur.getDate() - 1);
    }
    while ((this.byDate.get(ReportsView.keyOf(cur))?.total_duration ?? 0) > 0) {
      streak += 1;
      cur.setDate(cur.getDate() - 1);
    }
    return streak;
  }

  // ---- aggregations ---------------------------------------------------------

  private weekdayAverages(): { label: string; value: number }[] {
    const sums = new Array(7).fill(0);
    const counts = new Array(7).fill(0);
    for (const d of this.daysInPeriod()) {
      if (d.total > 0) {
        const idx = (d.date.getDay() + 6) % 7; // Monday = 0
        sums[idx] += d.total;
        counts[idx] += 1;
      }
    }
    return WEEKDAY_LABELS.map((label, i) => ({
      label,
      value: counts[i] > 0 ? Math.round(sums[i] / counts[i]) : 0,
    }));
  }

  // Active seconds attributed to each hour-of-day (0–23), split across the wall-clock
  // hours a lap actually spans, for laps that overlap the period.
  private hourHistogram(): number[] {
    const buckets = new Array(24).fill(0);
    const { start, end } = this.periodRange();
    const periodStart = start.getTime();
    const periodEnd = new Date(end.getFullYear(), end.getMonth(), end.getDate(), 23, 59, 59).getTime();

    for (const rec of this.records) {
      for (const lap of rec.laps) {
        if (lap.duration == null || lap.end_time == null) continue;
        let s = lap.start_time * 1000;
        let e = lap.end_time * 1000;
        if (e <= periodStart || s >= periodEnd) continue;
        s = Math.max(s, periodStart);
        e = Math.min(e, periodEnd);
        // Walk hour boundaries, crediting each hour its overlapping seconds.
        let cursor = s;
        while (cursor < e) {
          const d = new Date(cursor);
          const hourEnd = new Date(d.getFullYear(), d.getMonth(), d.getDate(), d.getHours() + 1, 0, 0).getTime();
          const segEnd = Math.min(e, hourEnd);
          buckets[d.getHours()] += (segEnd - cursor) / 1000;
          cursor = segEnd;
        }
      }
    }
    return buckets.map((v) => Math.round(v));
  }

  // ---- formatting -----------------------------------------------------------

  private static fmtDuration(seconds: number): string {
    const s = Math.max(0, Math.round(seconds));
    if (s === 0) return "0m";
    const h = Math.floor(s / 3600);
    const m = Math.round((s % 3600) / 60);
    if (h && m) return `${h}h ${m}m`;
    if (h) return `${h}h`;
    return `${m}m`;
  }

  // ---- render ---------------------------------------------------------------

  private render(): void {
    const days = this.daysInPeriod();
    const totals = days.map((d) => d.total);
    const total = totals.reduce((a, b) => a + b, 0);
    const trackedDays = totals.filter((t) => t > 0).length;
    const longest = Math.max(0, ...totals);
    const avgPerTracked = trackedDays > 0 ? total / trackedDays : 0;

    const hasData = total > 0;

    this.root.innerHTML = `
      <div class="reports-root">
        <div class="reports-controls">
          <div class="mode-switch" role="tablist">
            ${(["week", "month", "year"] as Mode[])
              .map(
                (m) =>
                  `<button class="mode-btn ${m === this.mode ? "active" : ""}" data-mode="${m}">${m[0].toUpperCase()}${m.slice(1)}</button>`,
              )
              .join("")}
          </div>
          <div class="period-nav">
            <button class="nav-btn" data-nav="-1" aria-label="Previous period">‹</button>
            <span class="period-label">${this.periodLabel()}</span>
            <button class="nav-btn" data-nav="1" aria-label="Next period" ${this.isCurrentPeriod() ? "disabled" : ""}>›</button>
          </div>
        </div>

        <div class="kpi-grid">
          ${this.kpiCard("Total", ReportsView.fmtDuration(total))}
          ${this.kpiCard("Avg / tracked day", ReportsView.fmtDuration(avgPerTracked))}
          ${this.kpiCard("Days tracked", `${trackedDays}`)}
          ${this.kpiCard("Longest day", ReportsView.fmtDuration(longest))}
          ${this.kpiCard("Current streak", `${this.currentStreak()}d`)}
        </div>

        ${hasData ? this.renderCharts(days) : `<div class="reports-empty">No activity recorded in this period.</div>`}
      </div>
    `;

    this.wireEvents();
    this.mountTooltip();
  }

  private kpiCard(label: string, value: string): string {
    return `
      <div class="kpi-card">
        <div class="kpi-value">${value}</div>
        <div class="kpi-label">${label}</div>
      </div>
    `;
  }

  private renderCharts(days: { date: Date; key: string; total: number }[]): string {
    let primary = "";
    if (this.mode === "week") {
      const bars = days.map((d) => ({
        label: d.date.toLocaleDateString(undefined, { weekday: "short" }),
        value: d.total,
        tip: `${d.date.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" })} · ${ReportsView.fmtDuration(d.total)}`,
        highlight: d.key === ReportsView.keyOf(new Date()),
      }));
      primary = this.chartCard("Daily total", this.columnChart(bars));
    } else if (this.mode === "month") {
      const bars = days.map((d) => ({
        label: `${d.date.getDate()}`,
        value: d.total,
        tip: `${d.date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} · ${ReportsView.fmtDuration(d.total)}`,
        highlight: d.key === ReportsView.keyOf(new Date()),
        sparseLabels: true,
      }));
      primary = this.chartCard("Daily total", this.columnChart(bars));
    } else {
      // Year: monthly totals.
      const monthTotals = new Array(12).fill(0);
      for (const d of days) monthTotals[d.date.getMonth()] += d.total;
      const bars = monthTotals.map((v, i) => ({
        label: MONTH_LABELS[i],
        value: v,
        tip: `${MONTH_LABELS[i]} ${this.anchor.getFullYear()} · ${ReportsView.fmtDuration(v)}`,
        highlight: this.isCurrentPeriod() && i === new Date().getMonth(),
      }));
      primary = this.chartCard("Monthly total", this.columnChart(bars));
    }

    const weekday =
      this.mode === "week"
        ? ""
        : this.chartCard("Average by weekday", this.columnChart(
            this.weekdayAverages().map((w) => ({
              label: w.label,
              value: w.value,
              tip: `${w.label} avg · ${ReportsView.fmtDuration(w.value)}`,
            })),
          ));

    const hours = this.chartCard("Active time by hour of day", this.hourHeatmap());

    const calendar = this.mode === "year" ? this.chartCard("Daily activity", this.calendarHeatmap()) : "";

    return primary + weekday + calendar + hours;
  }

  private chartCard(title: string, body: string): string {
    return `
      <div class="chart-card">
        <div class="chart-title">${title}</div>
        ${body}
      </div>
    `;
  }

  // Generic vertical column chart as inline SVG. Scales to container width via
  // viewBox; text uses ink tokens, bars use the single blue series hue.
  private columnChart(
    bars: { label: string; value: number; tip: string; highlight?: boolean; sparseLabels?: boolean }[],
  ): string {
    const n = bars.length;
    const W = 720;
    const H = 240;
    const padTop = 24;
    const padBottom = 30;
    const padLeft = 44;
    const padRight = 12;
    const plotH = H - padTop - padBottom;
    const plotW = W - padLeft - padRight;

    const maxV = Math.max(1, ...bars.map((b) => b.value));
    const top = ReportsView.niceTop(maxV);
    const y = (v: number) => padTop + plotH * (1 - v / top);
    const baseY = y(0);

    const slot = plotW / n;
    const barW = Math.min(24, slot * 0.62);

    // Gridlines + y labels at 0, half, top.
    const gridVals = [0, top / 2, top];
    const grid = gridVals
      .map((gv) => {
        const gy = y(gv);
        return `<line x1="${padLeft}" y1="${gy.toFixed(1)}" x2="${W - padRight}" y2="${gy.toFixed(1)}" class="viz-grid" />
          <text x="${padLeft - 8}" y="${(gy + 3.5).toFixed(1)}" class="viz-axis" text-anchor="end">${ReportsView.fmtDuration(gv)}</text>`;
      })
      .join("");

    const maxIdx = bars.reduce((mi, b, i) => (b.value > bars[mi].value ? i : mi), 0);

    const cols = bars
      .map((b, i) => {
        const cx = padLeft + slot * i + slot / 2;
        const bx = cx - barW / 2;
        const bh = b.value > 0 ? baseY - y(b.value) : 0;
        const by = baseY - bh;
        const cls = b.highlight ? "viz-bar highlight" : "viz-bar";
        const bar =
          bh > 0
            ? `<path d="${ReportsView.roundedTopBar(bx, by, barW, bh, 4)}" class="${cls}" data-tip="${ReportsView.esc(b.tip)}" />`
            : `<rect x="${bx.toFixed(1)}" y="${(baseY - 2).toFixed(1)}" width="${barW.toFixed(1)}" height="2" class="viz-bar-empty" data-tip="${ReportsView.esc(b.tip)}" />`;

        // Selective x labels: all when few, every 5th (day) when many.
        const showLabel = !b.sparseLabels || i === 0 || (i + 1) % 5 === 0 || i === n - 1;
        const xlabel = showLabel
          ? `<text x="${cx.toFixed(1)}" y="${(baseY + 18).toFixed(1)}" class="viz-axis" text-anchor="middle">${b.label}</text>`
          : "";

        // Direct-label only the tallest bar (skill: label selectively).
        const capLabel =
          i === maxIdx && b.value > 0
            ? `<text x="${cx.toFixed(1)}" y="${(by - 6).toFixed(1)}" class="viz-cap" text-anchor="middle">${ReportsView.fmtDuration(b.value)}</text>`
            : "";

        return bar + xlabel + capLabel;
      })
      .join("");

    return `<svg class="viz" viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid meet" role="img">
      ${grid}
      <line x1="${padLeft}" y1="${baseY}" x2="${W - padRight}" y2="${baseY}" class="viz-baseline" />
      ${cols}
    </svg>`;
  }

  // Single row of 24 hour cells, sequential blue ramp by intensity.
  private hourHeatmap(): string {
    const buckets = this.hourHistogram();
    const max = Math.max(1, ...buckets);
    const cells = buckets
      .map((v, h) => {
        const level = v <= 0 ? 0 : Math.min(4, Math.ceil((v / max) * 4));
        const hourLabel = `${h.toString().padStart(2, "0")}:00`;
        const tip = `${hourLabel} · ${ReportsView.fmtDuration(v)}`;
        const showTick = h % 3 === 0;
        return `<div class="hm-col">
          <div class="hm-cell lvl-${level}" data-tip="${ReportsView.esc(tip)}"></div>
          <div class="hm-tick">${showTick ? h : ""}</div>
        </div>`;
      })
      .join("");
    return `<div class="hour-heatmap">${cells}</div>
      <div class="hm-legend"><span>Less</span>${[0, 1, 2, 3, 4]
        .map((l) => `<span class="hm-swatch lvl-${l}"></span>`)
        .join("")}<span>More</span></div>`;
  }

  // GitHub-style contribution grid for the anchored year: columns = weeks,
  // rows = weekday (Mon top → Sun bottom), color by sequential blue level.
  private calendarHeatmap(): string {
    const year = this.anchor.getFullYear();
    const jan1 = new Date(year, 0, 1);
    const dec31 = new Date(year, 11, 31);

    // Grid starts on the Monday on/before Jan 1.
    const gridStart = ReportsView.startOfWeek(jan1);
    const max = Math.max(1, ...this.daysInPeriod().map((d) => d.total));

    const weeks: { key: string; total: number; inYear: boolean; month: number; date: Date }[][] = [];
    const cur = new Date(gridStart);
    while (cur <= dec31 || cur.getDay() !== 1) {
      const col: typeof weeks[number] = [];
      for (let r = 0; r < 7; r++) {
        const key = ReportsView.keyOf(cur);
        const inYear = cur.getFullYear() === year;
        col.push({
          key,
          total: inYear ? this.byDate.get(key)?.total_duration ?? 0 : -1,
          inYear,
          month: cur.getMonth(),
          date: new Date(cur),
        });
        cur.setDate(cur.getDate() + 1);
      }
      weeks.push(col);
      if (cur > dec31 && cur.getDay() === 1) break;
    }

    // Month labels: place a label above the first week whose Monday's month
    // differs from the previous week's.
    let prevMonth = -1;
    const monthRow = weeks
      .map((col) => {
        const m = col.find((c) => c.inYear)?.month ?? -1;
        if (m !== prevMonth && m !== -1) {
          prevMonth = m;
          return `<div class="cal-month">${MONTH_LABELS[m]}</div>`;
        }
        return `<div class="cal-month"></div>`;
      })
      .join("");

    const cols = weeks
      .map((col) => {
        const cells = col
          .map((c) => {
            if (!c.inYear) return `<div class="cal-cell empty"></div>`;
            const level = c.total <= 0 ? 0 : Math.min(4, Math.ceil((c.total / max) * 4));
            const tip = `${c.date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} · ${ReportsView.fmtDuration(c.total)}`;
            return `<div class="cal-cell lvl-${level}" data-tip="${ReportsView.esc(tip)}"></div>`;
          })
          .join("");
        return `<div class="cal-col">${cells}</div>`;
      })
      .join("");

    return `
      <div class="calendar-heatmap">
        <div class="cal-weekdays">
          <div>Mon</div><div></div><div>Wed</div><div></div><div>Fri</div><div></div><div>Sun</div>
        </div>
        <div class="cal-body">
          <div class="cal-months">${monthRow}</div>
          <div class="cal-grid">${cols}</div>
        </div>
      </div>
      <div class="hm-legend"><span>Less</span>${[0, 1, 2, 3, 4]
        .map((l) => `<span class="hm-swatch lvl-${l}"></span>`)
        .join("")}<span>More</span></div>`;
  }

  // ---- svg/util -------------------------------------------------------------

  // Path for a bar with rounded top corners and a square base (dataviz spec).
  private static roundedTopBar(x: number, y: number, w: number, h: number, r: number): string {
    const rr = Math.min(r, w / 2, h);
    return `M${x.toFixed(1)},${(y + h).toFixed(1)} V${(y + rr).toFixed(1)} Q${x.toFixed(1)},${y.toFixed(1)} ${(x + rr).toFixed(1)},${y.toFixed(1)} H${(x + w - rr).toFixed(1)} Q${(x + w).toFixed(1)},${y.toFixed(1)} ${(x + w).toFixed(1)},${(y + rr).toFixed(1)} V${(y + h).toFixed(1)} Z`;
  }

  // Round a seconds max up to a clean gridline top (15m…12h units).
  private static niceTop(maxSeconds: number): number {
    const units = [900, 1800, 3600, 7200, 10800, 14400, 21600, 28800, 43200, 86400];
    for (const u of units) {
      if (u * 2 >= maxSeconds) return Math.ceil(maxSeconds / (u / 2)) * (u / 2);
    }
    return Math.ceil(maxSeconds / 86400) * 86400;
  }

  private static esc(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  // ---- events + tooltip -----------------------------------------------------

  private wireEvents(): void {
    this.root.querySelectorAll<HTMLButtonElement>(".mode-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        this.mode = btn.dataset.mode as Mode;
        this.anchor = new Date(); // reset to the current period when switching scope
        this.render();
      });
    });
    this.root.querySelectorAll<HTMLButtonElement>(".nav-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        if (btn.disabled) return;
        this.shift(Number(btn.dataset.nav));
      });
    });
  }

  private mountTooltip(): void {
    let tip = this.root.querySelector<HTMLDivElement>(".viz-tooltip");
    if (!tip) {
      tip = document.createElement("div");
      tip.className = "viz-tooltip";
      this.root.appendChild(tip);
    }
    this.tooltip = tip;

    const rootEl = this.root;
    const onMove = (e: PointerEvent) => {
      const target = (e.target as Element)?.closest?.("[data-tip]") as HTMLElement | null;
      if (!target || !this.tooltip) {
        if (this.tooltip) this.tooltip.classList.remove("show");
        return;
      }
      const text = target.getAttribute("data-tip") || "";
      this.tooltip.textContent = text;
      this.tooltip.classList.add("show");
      const rect = rootEl.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      this.tooltip.style.left = `${x}px`;
      this.tooltip.style.top = `${y - 12}px`;
    };
    rootEl.addEventListener("pointermove", onMove);
    rootEl.addEventListener("pointerleave", () => this.tooltip?.classList.remove("show"));
  }
}
