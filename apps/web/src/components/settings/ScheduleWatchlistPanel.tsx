import { type FormEvent, useCallback, useEffect, useState } from 'react';
import { Bell, CalendarClock, CheckCircle2, Edit3, Plus, Save, Trash2, XCircle } from 'lucide-react';
import { ApiError, type ApiClient } from '../../api/client';
import { DEFAULTS } from '../../constants/routes';
import type {
  UpdateScheduleCmd,
  UpsertWatchlistRuleCmd,
  UserScheduleRecord,
  WatchlistRuleRecord,
} from '../../types/api';
import { AlertPriority } from '../../types/enums';

export interface ScheduleWatchlistPanelProps {
  api: ApiClient;
}

const PRIORITY_OPTIONS: { value: AlertPriority; label: string }[] = [
  { value: AlertPriority.NORMAL, label: 'Normal' },
  { value: AlertPriority.HIGH, label: 'High' },
  { value: AlertPriority.URGENT, label: 'Urgent' },
];

function formatError(err: unknown): string {
  if (err instanceof ApiError) {
    return err.requestId ? `${err.message} (Request ID: ${err.requestId})` : err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return 'Unexpected error';
}

function parseKeywords(raw: string): string[] {
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function getPriorityBadgeClass(priority: AlertPriority): string {
  switch (priority) {
    case AlertPriority.URGENT:
      return 'border-rose-500/40 bg-rose-500/15 text-rose-300';
    case AlertPriority.HIGH:
      return 'border-amber-500/40 bg-amber-500/15 text-amber-300';
    case AlertPriority.NORMAL:
      return 'border-slate-600/50 bg-slate-800 text-slate-300';
  }
}

export function ScheduleWatchlistPanel({ api }: ScheduleWatchlistPanelProps) {
  // Schedule State
  const [schedule, setSchedule] = useState<UserScheduleRecord | null>(null);
  const [timezone, setTimezone] = useState<string>(DEFAULTS.TIMEZONE);
  const [dailyTime, setDailyTime] = useState<string>(DEFAULTS.DAILY_TIME);
  const [emailEnabled, setEmailEnabled] = useState<boolean>(true);
  const [sendWhenEmpty, setSendWhenEmpty] = useState<boolean>(false);
  const [alertScanEnabled, setAlertScanEnabled] = useState<boolean>(true);
  const [alertIntervalMinutes, setAlertIntervalMinutes] = useState<string>(
    String(DEFAULTS.ALERT_INTERVAL_MINUTES),
  );
  const [savingSchedule, setSavingSchedule] = useState<boolean>(false);

  // Watchlist State
  const [rules, setRules] = useState<WatchlistRuleRecord[]>([]);
  const [newName, setNewName] = useState<string>('');
  const [newKeywords, setNewKeywords] = useState<string>('');
  const [newExcludeKeywords, setNewExcludeKeywords] = useState<string>('');
  const [newPriority, setNewPriority] = useState<AlertPriority>(AlertPriority.HIGH);
  const [newNotifyImmediately, setNewNotifyImmediately] = useState<boolean>(true);
  const [newEnabled, setNewEnabled] = useState<boolean>(true);
  const [creatingRule, setCreatingRule] = useState<boolean>(false);

  // Edit Rule State
  const [editingRuleId, setEditingRuleId] = useState<string | null>(null);
  const [editName, setEditName] = useState<string>('');
  const [editKeywords, setEditKeywords] = useState<string>('');
  const [editExcludeKeywords, setEditExcludeKeywords] = useState<string>('');
  const [editPriority, setEditPriority] = useState<AlertPriority>(AlertPriority.HIGH);
  const [editNotifyImmediately, setEditNotifyImmediately] = useState<boolean>(false);
  const [editEnabled, setEditEnabled] = useState<boolean>(true);
  const [savingRule, setSavingRule] = useState<boolean>(false);

  const [loading, setLoading] = useState<boolean>(true);
  const [busyRuleIds, setBusyRuleIds] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  const applySchedule = useCallback((rec: UserScheduleRecord) => {
    setSchedule(rec);
    setTimezone(rec.timezone);
    setDailyTime(rec.dailyTime);
    setEmailEnabled(rec.emailEnabled);
    setSendWhenEmpty(rec.sendWhenEmpty);
    setAlertScanEnabled(rec.alertScanEnabled);
    setAlertIntervalMinutes(String(rec.alertIntervalMinutes));
  }, []);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [sched, watchlist] = await Promise.all([api.getSchedule(), api.listWatchlist()]);
      applySchedule(sched);
      setRules(watchlist);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setLoading(false);
    }
  }, [api, applySchedule]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  const handleSaveSchedule = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    const tz = timezone.trim();
    const dt = dailyTime.trim();
    if (!tz) {
      setError('Timezone is required.');
      return;
    }
    if (!/^\d{2}:\d{2}$/.test(dt)) {
      setError('Daily Time must be in HH:MM 24-hour format.');
      return;
    }
    const [hh, mm] = dt.split(':').map(Number);
    if (hh < 0 || hh > 23 || mm < 0 || mm > 59) {
      setError('Daily Time must be between 00:00 and 23:59.');
      return;
    }
    const interval = Number(alertIntervalMinutes);
    if (!Number.isInteger(interval) || interval < 1) {
      setError('Alert Interval Minutes must be an integer >= 1.');
      return;
    }

    const cmd: UpdateScheduleCmd = {
      timezone: tz,
      dailyTime: dt,
      emailEnabled,
      sendWhenEmpty,
      alertScanEnabled,
      alertIntervalMinutes: interval,
    };

    setSavingSchedule(true);
    try {
      const updated = await api.updateSchedule(cmd);
      applySchedule(updated);
      setStatusMessage('Schedule settings saved.');
    } catch (err) {
      setError(formatError(err));
    } finally {
      setSavingSchedule(false);
    }
  };

  const handleAddRule = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    const keywords = parseKeywords(newKeywords);
    if (!newName.trim() || keywords.length === 0) {
      setError('Watchlist rule requires a name and at least one keyword.');
      return;
    }

    const cmd: UpsertWatchlistRuleCmd = {
      name: newName.trim(),
      keywords,
      excludeKeywords: parseKeywords(newExcludeKeywords),
      priority: newPriority,
      notifyImmediately: newNotifyImmediately,
      enabled: newEnabled,
    };

    setCreatingRule(true);
    try {
      const created = await api.createWatchlistRule(cmd);
      setRules((prev) => [created, ...prev]);
      setNewName('');
      setNewKeywords('');
      setNewExcludeKeywords('');
      setStatusMessage(`Created watchlist rule "${created.name}".`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setCreatingRule(false);
    }
  };

  const startEditRule = (rule: WatchlistRuleRecord) => {
    setEditingRuleId(rule.id);
    setEditName(rule.name);
    setEditKeywords(rule.keywords.join(', '));
    setEditExcludeKeywords(rule.excludeKeywords.join(', '));
    setEditPriority(rule.priority);
    setEditNotifyImmediately(rule.notifyImmediately);
    setEditEnabled(rule.enabled);
  };

  const handleSaveRuleEdit = async (ruleId: string) => {
    setError(null);
    setStatusMessage(null);

    const keywords = parseKeywords(editKeywords);
    if (!editName.trim() || keywords.length === 0) {
      setError('Watchlist rule requires a name and at least one keyword.');
      return;
    }

    const cmd: UpsertWatchlistRuleCmd = {
      name: editName.trim(),
      keywords,
      excludeKeywords: parseKeywords(editExcludeKeywords),
      priority: editPriority,
      notifyImmediately: editNotifyImmediately,
      enabled: editEnabled,
    };

    setSavingRule(true);
    try {
      const updated = await api.updateWatchlistRule(ruleId, cmd);
      setRules((prev) => prev.map((r) => (r.id === updated.id ? updated : r)));
      setEditingRuleId(null);
      setStatusMessage(`Updated watchlist rule "${updated.name}".`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setSavingRule(false);
    }
  };

  const handleToggleRule = async (rule: WatchlistRuleRecord) => {
    setError(null);
    setBusyRuleIds((prev) => ({ ...prev, [rule.id]: true }));
    try {
      const cmd: UpsertWatchlistRuleCmd = {
        name: rule.name,
        keywords: rule.keywords,
        excludeKeywords: rule.excludeKeywords ?? [],
        priority: rule.priority,
        notifyImmediately: rule.notifyImmediately,
        enabled: !rule.enabled,
      };
      const updated = await api.updateWatchlistRule(rule.id, cmd);
      setRules((prev) => prev.map((r) => (r.id === updated.id ? updated : r)));
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyRuleIds((prev) => ({ ...prev, [rule.id]: false }));
    }
  };

  const handleDeleteRule = async (rule: WatchlistRuleRecord) => {
    setError(null);
    setStatusMessage(null);
    setBusyRuleIds((prev) => ({ ...prev, [rule.id]: true }));
    try {
      await api.deleteWatchlistRule(rule.id);
      setRules((prev) => prev.filter((r) => r.id !== rule.id));
      if (editingRuleId === rule.id) {
        setEditingRuleId(null);
      }
      setStatusMessage(`Deleted watchlist rule "${rule.name}".`);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setBusyRuleIds((prev) => ({ ...prev, [rule.id]: false }));
    }
  };

  if (loading && !schedule) {
    return (
      <div className="rounded-xl border border-slate-800 bg-slate-900/70 p-6 text-sm text-slate-400">
        Loading schedule and watchlist rules...
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {error && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-rose-500/40 bg-rose-950/50 px-4 py-3 text-sm text-rose-200"
        >
          <XCircle className="mt-0.5 h-4 w-4 shrink-0 text-rose-400" />
          <span>{error}</span>
        </div>
      )}

      {statusMessage && (
        <div
          role="status"
          className="flex items-start gap-2 rounded-lg border border-emerald-500/40 bg-emerald-950/40 px-4 py-3 text-sm text-emerald-200"
        >
          <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" />
          <span>{statusMessage}</span>
        </div>
      )}

      {/* Schedule Section */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div className="flex items-center gap-2">
          <CalendarClock className="h-5 w-5 text-indigo-400" />
          <div>
            <h2 className="text-base font-semibold text-white">Daily Schedule &amp; Alerts</h2>
            <p className="text-xs text-slate-400">
              Control daily digest timing, master delivery switch, and alert scanner cadence.
            </p>
          </div>
        </div>

        <form onSubmit={(e) => void handleSaveSchedule(e)} className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-3">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Timezone (IANA)</span>
              <input
                type="text"
                value={timezone}
                onChange={(e) => setTimezone(e.target.value)}
                required
                placeholder="Asia/Shanghai"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Daily Time (HH:MM)</span>
              <input
                type="text"
                value={dailyTime}
                onChange={(e) => setDailyTime(e.target.value)}
                required
                placeholder="08:30"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">Alert Scan Interval (minutes)</span>
              <input
                type="number"
                min="1"
                step="1"
                value={alertIntervalMinutes}
                onChange={(e) => setAlertIntervalMinutes(e.target.value)}
                required
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>
          </div>

          <div className="flex flex-wrap items-center gap-6 text-xs text-slate-300">
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={emailEnabled}
                onChange={(e) => setEmailEnabled(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>Notifications Master Switch (emailEnabled)</span>
            </label>
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={sendWhenEmpty}
                onChange={(e) => setSendWhenEmpty(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>Send Even When Empty</span>
            </label>
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={alertScanEnabled}
                onChange={(e) => setAlertScanEnabled(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>Enable High-Frequency Alert Scan</span>
            </label>
          </div>

          <button
            type="submit"
            disabled={savingSchedule}
            className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
          >
            <Save className="h-4 w-4" />
            {savingSchedule ? 'Saving...' : 'Save Schedule'}
          </button>
        </form>
      </section>

      {/* Watchlist Rules Section */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-5">
        <div className="flex items-center gap-2">
          <Bell className="h-5 w-5 text-amber-400" />
          <div>
            <h2 className="text-base font-semibold text-white">Watchlist Alert Rules</h2>
            <p className="text-xs text-slate-400">
              Highlight matching items and trigger immediate alerts for urgent keywords.
            </p>
          </div>
        </div>

        <form
          onSubmit={(e) => void handleAddRule(e)}
          className="space-y-4 rounded-lg border border-slate-800 bg-slate-950/50 p-4"
        >
          <div className="grid gap-4 sm:grid-cols-2">
            <label className="block space-y-1 text-xs">
              <span className="font-medium text-slate-200">Rule Name</span>
              <input
                type="text"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                required
                placeholder="e.g. Gemini 2.5 Releases"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>

            <label className="block space-y-1 text-xs">
              <span className="font-medium text-slate-200">Priority</span>
              <select
                value={newPriority}
                onChange={(e) => setNewPriority(e.target.value as AlertPriority)}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              >
                {PRIORITY_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <div className="grid gap-4 sm:grid-cols-2">
            <label className="block space-y-1 text-xs">
              <span className="font-medium text-slate-200">Keywords (comma-separated)</span>
              <input
                type="text"
                value={newKeywords}
                onChange={(e) => setNewKeywords(e.target.value)}
                required
                placeholder="gemini 2.5, thinking, multimodal"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>

            <label className="block space-y-1 text-xs">
              <span className="font-medium text-slate-200">
                Exclude Keywords (optional, comma-separated)
              </span>
              <input
                type="text"
                value={newExcludeKeywords}
                onChange={(e) => setNewExcludeKeywords(e.target.value)}
                placeholder="rumor, spam"
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>
          </div>

          <div className="flex flex-wrap items-center gap-6 text-xs text-slate-300">
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={newNotifyImmediately}
                onChange={(e) => setNewNotifyImmediately(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>Notify Immediately</span>
            </label>
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={newEnabled}
                onChange={(e) => setNewEnabled(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>Rule Enabled</span>
            </label>
          </div>

          <button
            type="submit"
            disabled={creatingRule}
            className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
          >
            <Plus className="h-4 w-4" />
            {creatingRule ? 'Adding...' : 'Add Watchlist Rule'}
          </button>
        </form>

        {/* Rules List */}
        <div className="space-y-3">
          {rules.length === 0 ? (
            <p className="py-3 text-sm text-slate-500">No watchlist rules configured yet.</p>
          ) : (
            rules.map((rule) => {
              const isEditing = editingRuleId === rule.id;
              const isBusy = Boolean(busyRuleIds[rule.id]);

              return (
                <div
                  key={rule.id}
                  className="rounded-lg border border-slate-800 bg-slate-950/60 p-4 space-y-3"
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-sm font-semibold text-white">{rule.name}</span>
                        <span
                          className={`rounded-full border px-2 py-0.5 text-[11px] font-medium ${getPriorityBadgeClass(rule.priority)}`}
                        >
                          {rule.priority}
                        </span>
                        {rule.notifyImmediately && (
                          <span className="rounded border border-indigo-500/30 bg-indigo-500/10 px-2 py-0.5 text-[11px] text-indigo-300">
                            Immediate Alert
                          </span>
                        )}
                      </div>
                      <div className="text-xs text-slate-300">
                        Keywords: <span className="font-mono">{rule.keywords.join(', ')}</span>
                      </div>
                      {rule.excludeKeywords.length > 0 && (
                        <div className="text-xs text-slate-400">
                          Exclude:{' '}
                          <span className="font-mono">{rule.excludeKeywords.join(', ')}</span>
                        </div>
                      )}
                    </div>

                    <div className="flex flex-wrap items-center gap-2">
                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleToggleRule(rule)}
                        className={`rounded-lg border px-2.5 py-1.5 text-xs font-medium transition disabled:opacity-50 ${
                          rule.enabled
                            ? 'border-emerald-500/40 bg-emerald-500/15 text-emerald-200 hover:bg-emerald-500/25'
                            : 'border-slate-700 bg-slate-800 text-slate-300 hover:bg-slate-700'
                        }`}
                      >
                        {rule.enabled ? 'Enabled' : 'Disabled'}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() =>
                          isEditing ? setEditingRuleId(null) : startEditRule(rule)
                        }
                        className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:opacity-50"
                      >
                        <Edit3 className="h-3.5 w-3.5" />
                        {isEditing ? 'Cancel' : 'Edit'}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleDeleteRule(rule)}
                        className="inline-flex items-center gap-1 rounded-lg border border-rose-500/40 bg-rose-500/10 px-2.5 py-1.5 text-xs font-medium text-rose-200 hover:bg-rose-500/20 disabled:opacity-50"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        Delete
                      </button>
                    </div>
                  </div>

                  {isEditing && (
                    <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/70 p-4">
                      <div className="grid gap-3 sm:grid-cols-2">
                        <label className="block space-y-1 text-xs">
                          <span className="font-medium text-slate-200">Rule Name</span>
                          <input
                            type="text"
                            value={editName}
                            onChange={(e) => setEditName(e.target.value)}
                            className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                          />
                        </label>
                        <label className="block space-y-1 text-xs">
                          <span className="font-medium text-slate-200">Priority</span>
                          <select
                            value={editPriority}
                            onChange={(e) => setEditPriority(e.target.value as AlertPriority)}
                            className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                          >
                            {PRIORITY_OPTIONS.map((opt) => (
                              <option key={opt.value} value={opt.value}>
                                {opt.label}
                              </option>
                            ))}
                          </select>
                        </label>
                      </div>

                      <div className="grid gap-3 sm:grid-cols-2">
                        <label className="block space-y-1 text-xs">
                          <span className="font-medium text-slate-200">Keywords</span>
                          <input
                            type="text"
                            value={editKeywords}
                            onChange={(e) => setEditKeywords(e.target.value)}
                            className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                          />
                        </label>
                        <label className="block space-y-1 text-xs">
                          <span className="font-medium text-slate-200">Exclude Keywords</span>
                          <input
                            type="text"
                            value={editExcludeKeywords}
                            onChange={(e) => setEditExcludeKeywords(e.target.value)}
                            className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                          />
                        </label>
                      </div>

                      <div className="flex flex-wrap items-center gap-5 text-xs text-slate-300">
                        <label className="inline-flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={editNotifyImmediately}
                            onChange={(e) => setEditNotifyImmediately(e.target.checked)}
                            className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                          />
                          <span>Notify Immediately</span>
                        </label>
                        <label className="inline-flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={editEnabled}
                            onChange={(e) => setEditEnabled(e.target.checked)}
                            className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                          />
                          <span>Enabled</span>
                        </label>
                      </div>

                      <button
                        type="button"
                        disabled={savingRule}
                        onClick={() => void handleSaveRuleEdit(rule.id)}
                        className="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
                      >
                        {savingRule ? 'Saving...' : 'Save Rule'}
                      </button>
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
      </section>
    </div>
  );
}
