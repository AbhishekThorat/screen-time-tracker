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
  private selectedKey: string | null = null; // day whose lap breakdown is shown
  private eventsInit = false;

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

  private static parseKey(key: string): Date {
    const [y, m, d] = key.split("-").map(Number);
    return new Date(y, m - 1, d);
  }

  // "Mon, Jul 6" with a Today / Yesterday suffix when applicable.
  private static dayLabel(key: string): string {
    const today = ReportsView.keyOf(new Date());
    const y = new Date();
    y.setDate(y.getDate() - 1);
    const yesterday = ReportsView.keyOf(y);
    let suffix = "";
    if (key === today) suffix = " · Today";
    else if (key === yesterday) suffix = " · Yesterday";
    const pretty = ReportsView.parseKey(key).toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
    });
    return `${pretty}${suffix}`;
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

  // HH:MM:SS — used for individual lap durations in the day-detail panel.
  private static fmtClock(seconds: number): string {
    const s = Math.max(0, Math.round(seconds));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    return `${h.toString().padStart(2, "0")}:${m.toString().padStart(2, "0")}:${sec.toString().padStart(2, "0")}`;
  }

  // ---- render ---------------------------------------------------------------

  private render(): void {
    this.initEvents();

    const days = this.daysInPeriod();
    const totals = days.map((d) => d.total);
    const total = totals.reduce((a, b) => a + b, 0);
    const trackedDays = totals.filter((t) => t > 0).length;
    const longest = Math.max(0, ...totals);
    const avgPerTracked = trackedDays > 0 ? total / trackedDays : 0;

    // Keep a valid day selected for the detail panel: if the current selection
    // isn't in this period, default to the most recent day that has data.
    if (this.selectedKey === null || !days.some((d) => d.key === this.selectedKey)) {
      const withData = days.filter((d) => d.total > 0);
      this.selectedKey = withData.length ? withData[withData.length - 1].key : null;
    }

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
    const hint = '<span class="chart-hint">click a bar for details</span>';
    if (this.mode === "week") {
      const bars = days.map((d) => ({
        label: d.date.toLocaleDateString(undefined, { weekday: "short" }),
        value: d.total,
        tip: `${d.date.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" })} · ${ReportsView.fmtDuration(d.total)}`,
        dayKey: d.key,
      }));
      primary = this.chartCard("Daily total", this.columnChart(bars), hint);
    } else if (this.mode === "month") {
      const bars = days.map((d) => ({
        label: `${d.date.getDate()}`,
        value: d.total,
        tip: `${d.date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} · ${ReportsView.fmtDuration(d.total)}`,
        dayKey: d.key,
        sparseLabels: true,
      }));
      primary = this.chartCard("Daily total", this.columnChart(bars), hint);
    } else {
      // Year: monthly totals. Clicking a month bar drills into that month.
      const monthTotals = new Array(12).fill(0);
      for (const d of days) monthTotals[d.date.getMonth()] += d.total;
      const bars = monthTotals.map((v, i) => ({
        label: MONTH_LABELS[i],
        value: v,
        tip: `${MONTH_LABELS[i]} ${this.anchor.getFullYear()} · ${ReportsView.fmtDuration(v)}`,
        monthIdx: i,
        emphasize: this.isCurrentPeriod() && i === new Date().getMonth(),
      }));
      primary = this.chartCard("Monthly total", this.columnChart(bars), '<span class="chart-hint">click a month to drill in</span>');
    }

    // Day-detail panel (the drill-down target; replaces the old History list).
    const detail = this.selectedKey ? this.renderDayDetail(this.selectedKey) : "";

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

    const calendar =
      this.mode === "year"
        ? this.chartCard("Daily activity", this.calendarHeatmap(), '<span class="chart-hint">click a day for details</span>')
        : "";

    return primary + detail + weekday + calendar + hours;
  }

  private chartCard(title: string, body: string, hint = ""): string {
    return `
      <div class="chart-card">
        <div class="chart-title">${title}${hint}</div>
        ${body}
      </div>
    `;
  }

  // Detail panel for one day: its lap breakdown. This is the drill-down target
  // that replaced the always-on History list.
  private renderDayDetail(key: string): string {
    const rec = this.byDate.get(key);
    const laps = rec?.laps ?? [];
    const completed = laps.filter((l) => l.duration != null && l.duration > 0);
    const total = rec?.total_duration ?? 0;
    const activeBadge = rec?.is_active ? '<span class="history-badge active">Active</span>' : "";

    const lapsHtml =
      laps.length === 0
        ? '<div class="empty-laps">No laps recorded this day</div>'
        : laps
            .map((lap, i) => {
              const start = new Date(lap.start_time * 1000);
              const end = lap.end_time ? new Date(lap.end_time * 1000) : null;
              const ongoing = lap.duration == null;
              const dur = ongoing ? "Ongoing" : ReportsView.fmtClock(lap.duration || 0);
              return `<div class="history-lap ${ongoing ? "ongoing" : ""}">
                <span class="history-lap-num">Lap ${i + 1}</span>
                <span class="history-lap-period">${start.toLocaleTimeString()} - ${end ? end.toLocaleTimeString() : "Ongoing"}</span>
                <span class="history-lap-duration">${dur}</span>
              </div>`;
            })
            .join("");

    return `
      <div class="chart-card day-detail">
        <div class="day-detail-header">
          <div class="day-detail-title">${ReportsView.dayLabel(key)} ${activeBadge}</div>
          <div class="day-detail-meta">${ReportsView.fmtDuration(total)} · ${completed.length} lap${completed.length === 1 ? "" : "s"}</div>
        </div>
        <div class="day-detail-laps">${lapsHtml}</div>
      </div>
    `;
  }

  // Generic vertical column chart as inline SVG. Scales to container width via
  // viewBox; text uses ink tokens, bars use the single blue series hue.
  // Bars carrying `dayKey` (drill to that day's laps) or `monthIdx` (drill into
  // that month) become clickable, with a full-height transparent hit target.
  private columnChart(
    bars: {
      label: string;
      value: number;
      tip: string;
      dayKey?: string;
      monthIdx?: number;
      emphasize?: boolean;
      sparseLabels?: boolean;
    }[],
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
        const slotLeft = padLeft + slot * i;
        const cx = slotLeft + slot / 2;
        const bx = cx - barW / 2;
        const bh = b.value > 0 ? baseY - y(b.value) : 0;
        const by = baseY - bh;
        const clickable = b.dayKey != null || b.monthIdx != null;
        const strong = (b.dayKey != null && b.dayKey === this.selectedKey) || !!b.emphasize;
        const cls = `viz-bar${strong ? " selected" : ""}${clickable ? " clickable" : ""}`;
        const bar =
          bh > 0
            ? `<path d="${ReportsView.roundedTopBar(bx, by, barW, bh, 4)}" class="${cls}" data-tip="${ReportsView.esc(b.tip)}" />`
            : `<rect x="${bx.toFixed(1)}" y="${(baseY - 2).toFixed(1)}" width="${barW.toFixed(1)}" height="2" class="viz-bar-empty" data-tip="${ReportsView.esc(b.tip)}" />`;

        // Full-height transparent hit target so the whole column is clickable/hoverable.
        const drill = b.dayKey != null ? `data-day="${b.dayKey}"` : b.monthIdx != null ? `data-month="${b.monthIdx}"` : "";
        const hit = clickable
          ? `<rect class="viz-hit" x="${slotLeft.toFixed(1)}" y="${padTop}" width="${slot.toFixed(1)}" height="${(baseY - padTop).toFixed(1)}" data-tip="${ReportsView.esc(b.tip)}" ${drill} />`
          : "";

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

        return bar + hit + xlabel + capLabel;
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
            const sel = c.key === this.selectedKey ? " selected" : "";
            return `<div class="cal-cell lvl-${level} clickable${sel}" data-tip="${ReportsView.esc(tip)}" data-day="${c.key}"></div>`;
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

  // Attached ONCE to the persistent root. All controls are handled by delegation
  // so re-rendering (which replaces root.innerHTML) never accumulates listeners.
  private initEvents(): void {
    if (this.eventsInit) return;
    this.eventsInit = true;

    // Tooltip lives on <body> (position: fixed) so it survives innerHTML swaps.
    const tip = document.createElement("div");
    tip.className = "viz-tooltip";
    document.body.appendChild(tip);
    this.tooltip = tip;

    this.root.addEventListener("click", (e) => {
      const t = e.target as Element;

      const modeEl = t.closest<HTMLElement>("[data-mode]");
      if (modeEl) {
        this.mode = modeEl.dataset.mode as Mode;
        this.anchor = new Date(); // reset to the current period when switching scope
        this.selectedKey = null;
        this.render();
        return;
      }

      const navEl = t.closest<HTMLButtonElement>("[data-nav]");
      if (navEl) {
        if (navEl.disabled) return;
        this.shift(Number(navEl.dataset.nav));
        return;
      }

      const monthEl = t.closest<HTMLElement>("[data-month]");
      if (monthEl) {
        this.mode = "month";
        this.anchor = new Date(this.anchor.getFullYear(), Number(monthEl.dataset.month), 1);
        this.selectedKey = null; // re-defaults to the month's most recent day
        this.render();
        return;
      }

      const dayEl = t.closest<HTMLElement>("[data-day]");
      if (dayEl) {
        this.selectedKey = dayEl.dataset.day ?? null;
        this.render();
        return;
      }
    });

    this.root.addEventListener("pointermove", (e) => {
      const target = (e.target as Element)?.closest?.("[data-tip]") as HTMLElement | null;
      if (!target || !this.tooltip) {
        this.tooltip?.classList.remove("show");
        return;
      }
      this.tooltip.textContent = target.getAttribute("data-tip") || "";
      this.tooltip.classList.add("show");
      this.tooltip.style.left = `${(e as PointerEvent).clientX}px`;
      this.tooltip.style.top = `${(e as PointerEvent).clientY - 12}px`;
    });
    this.root.addEventListener("pointerleave", () => this.tooltip?.classList.remove("show"));
  }
}
