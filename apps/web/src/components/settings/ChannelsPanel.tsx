import { type FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import { CheckCircle2, Edit3, Plus, Send, Trash2, XCircle } from 'lucide-react';
import { ApiError, type ApiClient } from '../../api/client';
import { DEFAULTS } from '../../constants/routes';
import { type TranslateFn, useI18n } from '../../i18n';
import type {
  ChannelConfigPayload,
  ChannelTestResult,
  CreateChannelCmd,
  NotificationChannelView,
  UpdateChannelCmd,
} from '../../types/api';
import { DeliveryStatus, NotificationChannelType } from '../../types/enums';

export interface ChannelsPanelProps {
  api: ApiClient;
}

const CHANNEL_TYPE_VALUES: readonly NotificationChannelType[] = [
  NotificationChannelType.SMTP,
  NotificationChannelType.TELEGRAM,
  NotificationChannelType.FEISHU,
  NotificationChannelType.DINGTALK,
  NotificationChannelType.WECOM,
  NotificationChannelType.WEBHOOK,
];

function formatError(err: unknown, t: TranslateFn): string {
  if (err instanceof ApiError) {
    return err.requestId
      ? `${err.message} (${t('common.requestId', { id: err.requestId })})`
      : err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return t('common.unexpectedError');
}

interface ChannelFormFields {
  // SMTP
  smtpHost: string;
  smtpPort: string;
  smtpUsername: string;
  smtpPassword: string;
  smtpFrom: string;
  smtpTo: string;
  smtpUseStarttls: boolean;
  smtpUseTls: boolean;
  // Telegram
  tgBotToken: string;
  tgChatId: string;
  tgApiBaseUrl: string;
  // Feishu / DingTalk / WeCom
  webhookUrl: string;
  webhookSecret: string;
  // Generic Webhook
  genericUrl: string;
  genericSecret: string;
}

function emptyChannelFields(): ChannelFormFields {
  return {
    smtpHost: '',
    smtpPort: String(DEFAULTS.SMTP_PORT),
    smtpUsername: '',
    smtpPassword: '',
    smtpFrom: '',
    smtpTo: '',
    smtpUseStarttls: true,
    smtpUseTls: false,
    tgBotToken: '',
    tgChatId: '',
    tgApiBaseUrl: '',
    webhookUrl: '',
    webhookSecret: '',
    genericUrl: '',
    genericSecret: '',
  };
}

function buildConfigPayload(
  channelType: NotificationChannelType,
  fields: ChannelFormFields,
  t: TranslateFn,
): { payload: ChannelConfigPayload | null; validationError: string | null } {
  switch (channelType) {
    case NotificationChannelType.SMTP: {
      const host = fields.smtpHost.trim();
      const from = fields.smtpFrom.trim();
      const to = fields.smtpTo
        .split(',')
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      const port = Number(fields.smtpPort);
      if (!host || !from || to.length === 0) {
        return {
          payload: null,
          validationError: t('channels.smtpRequired'),
        };
      }
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        return { payload: null, validationError: t('channels.smtpPortRange') };
      }
      return {
        payload: {
          host,
          port,
          username: fields.smtpUsername.trim() || undefined,
          password: fields.smtpPassword || undefined,
          from,
          to,
          useStarttls: fields.smtpUseStarttls,
          useTls: fields.smtpUseTls,
        },
        validationError: null,
      };
    }

    case NotificationChannelType.TELEGRAM: {
      const botToken = fields.tgBotToken.trim();
      const chatId = fields.tgChatId.trim();
      if (!botToken || !chatId) {
        return {
          payload: null,
          validationError: t('channels.telegramRequired'),
        };
      }
      return {
        payload: {
          botToken,
          chatId,
          apiBaseUrl: fields.tgApiBaseUrl.trim() || undefined,
        },
        validationError: null,
      };
    }

    case NotificationChannelType.FEISHU: {
      const webhookUrl = fields.webhookUrl.trim();
      if (!webhookUrl) {
        return { payload: null, validationError: t('channels.feishuRequired') };
      }
      return {
        payload: {
          webhookUrl,
          secret: fields.webhookSecret.trim() || undefined,
        },
        validationError: null,
      };
    }

    case NotificationChannelType.DINGTALK: {
      const webhookUrl = fields.webhookUrl.trim();
      if (!webhookUrl) {
        return { payload: null, validationError: t('channels.dingtalkRequired') };
      }
      return {
        payload: {
          webhookUrl,
          secret: fields.webhookSecret.trim() || undefined,
        },
        validationError: null,
      };
    }

    case NotificationChannelType.WECOM: {
      const webhookUrl = fields.webhookUrl.trim();
      if (!webhookUrl) {
        return { payload: null, validationError: t('channels.wecomRequired') };
      }
      return {
        payload: { webhookUrl },
        validationError: null,
      };
    }

    case NotificationChannelType.WEBHOOK: {
      const url = fields.genericUrl.trim();
      if (!url) {
        return { payload: null, validationError: t('channels.webhookRequired') };
      }
      return {
        payload: {
          url,
          secret: fields.genericSecret.trim() || undefined,
        },
        validationError: null,
      };
    }
  }
}

interface DynamicChannelFieldsProps {
  channelType: NotificationChannelType;
  fields: ChannelFormFields;
  onChange: (patch: Partial<ChannelFormFields>) => void;
}

function DynamicChannelConfigFields({ channelType, fields, onChange }: DynamicChannelFieldsProps) {
  const { t } = useI18n();
  switch (channelType) {
    case NotificationChannelType.SMTP:
      return (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.smtpHost')}</span>
            <input
              type="text"
              value={fields.smtpHost}
              onChange={(e) => onChange({ smtpHost: e.target.value })}
              placeholder="smtp.example.com"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.smtpPort')}</span>
            <input
              type="number"
              min="1"
              max="65535"
              value={fields.smtpPort}
              onChange={(e) => {
                const nextPort = e.target.value;
                const trimmed = nextPort.trim();
                if (trimmed === String(DEFAULTS.SMTP_IMPLICIT_TLS_PORT)) {
                  onChange({ smtpPort: nextPort, smtpUseTls: true, smtpUseStarttls: false });
                } else if (trimmed === String(DEFAULTS.SMTP_PORT)) {
                  onChange({ smtpPort: nextPort, smtpUseStarttls: true, smtpUseTls: false });
                } else {
                  onChange({ smtpPort: nextPort });
                }
              }}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.smtpUsername')}</span>
            <input
              type="text"
              value={fields.smtpUsername}
              onChange={(e) => onChange({ smtpUsername: e.target.value })}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.smtpPassword')}</span>
            <input
              type="password"
              autoComplete="new-password"
              value={fields.smtpPassword}
              onChange={(e) => onChange({ smtpPassword: e.target.value })}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.smtpFrom')}</span>
            <input
              type="email"
              value={fields.smtpFrom}
              onChange={(e) => onChange({ smtpFrom: e.target.value })}
              placeholder="digest@example.com"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.smtpTo')}</span>
            <input
              type="text"
              value={fields.smtpTo}
              onChange={(e) => onChange({ smtpTo: e.target.value })}
              placeholder="alice@example.com, bob@example.com"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <div className="flex flex-wrap items-center gap-4 sm:col-span-2">
            <label className="inline-flex items-center gap-2 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={fields.smtpUseStarttls}
                onChange={(e) =>
                  onChange(
                    e.target.checked
                      ? { smtpUseStarttls: true, smtpUseTls: false }
                      : { smtpUseStarttls: false },
                  )
                }
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>{t('channels.smtpStarttls')}</span>
            </label>
            <label className="inline-flex items-center gap-2 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={fields.smtpUseTls}
                onChange={(e) =>
                  onChange(
                    e.target.checked
                      ? { smtpUseTls: true, smtpUseStarttls: false }
                      : { smtpUseTls: false },
                  )
                }
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>{t('channels.smtpTls')}</span>
            </label>
          </div>
        </div>
      );

    case NotificationChannelType.TELEGRAM:
      return (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.tgBotToken')}</span>
            <input
              type="password"
              autoComplete="new-password"
              value={fields.tgBotToken}
              onChange={(e) => onChange({ tgBotToken: e.target.value })}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.tgChatId')}</span>
            <input
              type="text"
              value={fields.tgChatId}
              onChange={(e) => onChange({ tgChatId: e.target.value })}
              placeholder="-1001234567890"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs sm:col-span-2">
            <span className="font-medium text-slate-200">{t('channels.tgApiBaseUrl')}</span>
            <input
              type="url"
              value={fields.tgApiBaseUrl}
              onChange={(e) => onChange({ tgApiBaseUrl: e.target.value })}
              placeholder="https://api.telegram.org"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
        </div>
      );

    case NotificationChannelType.FEISHU:
    case NotificationChannelType.DINGTALK:
      return (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.webhookUrl')}</span>
            <input
              type="url"
              value={fields.webhookUrl}
              onChange={(e) => onChange({ webhookUrl: e.target.value })}
              placeholder="https://open.feishu.cn/open-apis/bot/v2/hook/..."
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.signingSecret')}</span>
            <input
              type="password"
              autoComplete="new-password"
              value={fields.webhookSecret}
              onChange={(e) => onChange({ webhookSecret: e.target.value })}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
        </div>
      );

    case NotificationChannelType.WECOM:
      return (
        <label className="block space-y-1 text-xs">
          <span className="font-medium text-slate-200">{t('channels.wecomWebhookUrl')}</span>
          <input
            type="url"
            value={fields.webhookUrl}
            onChange={(e) => onChange({ webhookUrl: e.target.value })}
            placeholder="https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=..."
            className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
          />
        </label>
      );

    case NotificationChannelType.WEBHOOK:
      return (
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.webhookUrl')}</span>
            <input
              type="url"
              value={fields.genericUrl}
              onChange={(e) => onChange({ genericUrl: e.target.value })}
              placeholder="https://hooks.example.com/digest"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
          <label className="block space-y-1 text-xs">
            <span className="font-medium text-slate-200">{t('channels.hmacSecret')}</span>
            <input
              type="password"
              autoComplete="new-password"
              value={fields.genericSecret}
              onChange={(e) => onChange({ genericSecret: e.target.value })}
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
            />
          </label>
        </div>
      );
  }
}

export function ChannelsPanel({ api }: ChannelsPanelProps) {
  const { t, formatDateTime, labelChannelType, labelDeliveryStatus } = useI18n();
  const tRef = useRef(t);
  tRef.current = t;

  const [channels, setChannels] = useState<NotificationChannelView[]>([]);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  // Add Form
  const [addType, setAddType] = useState<NotificationChannelType>(NotificationChannelType.SMTP);
  const [addName, setAddName] = useState<string>('');
  const [addTriggerDaily, setAddTriggerDaily] = useState<boolean>(true);
  const [addTriggerAlert, setAddTriggerAlert] = useState<boolean>(true);
  const [addFields, setAddFields] = useState<ChannelFormFields>(emptyChannelFields);
  const [creating, setCreating] = useState<boolean>(false);

  // Edit Form
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState<string>('');
  const [editTriggerDaily, setEditTriggerDaily] = useState<boolean>(true);
  const [editTriggerAlert, setEditTriggerAlert] = useState<boolean>(true);
  const [editUpdateConfig, setEditUpdateConfig] = useState<boolean>(false);
  const [editFields, setEditFields] = useState<ChannelFormFields>(emptyChannelFields);
  const [savingEdit, setSavingEdit] = useState<boolean>(false);

  // Per-channel busy + test results
  const [busyIds, setBusyIds] = useState<Record<string, boolean>>({});
  const [testResults, setTestResults] = useState<Record<string, ChannelTestResult>>({});

  const loadChannels = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await api.listChannels();
      setChannels(list);
    } catch (err) {
      setError(formatError(err, tRef.current));
    } finally {
      setLoading(false);
    }
  }, [api]);

  useEffect(() => {
    void loadChannels();
  }, [loadChannels]);

  const handleAddChannel = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    setStatusMessage(null);

    if (!addName.trim()) {
      setError(t('channels.nameRequired'));
      return;
    }

    const { payload, validationError } = buildConfigPayload(addType, addFields, t);
    if (validationError || !payload) {
      setError(validationError ?? t('channels.invalidConfig'));
      return;
    }

    const cmd: CreateChannelCmd = {
      channelType: addType,
      name: addName.trim(),
      enabled: true,
      config: payload,
      triggerOnDaily: addTriggerDaily,
      triggerOnAlert: addTriggerAlert,
    };

    setCreating(true);
    try {
      const created = await api.createChannel(cmd);
      setChannels((prev) => [created, ...prev]);
      setAddName('');
      setAddFields(emptyChannelFields());
      setStatusMessage(t('channels.createdMsg', { name: created.name }));
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setCreating(false);
    }
  };

  const handleToggleEnabled = async (channel: NotificationChannelView) => {
    setError(null);
    setBusyIds((prev) => ({ ...prev, [channel.id]: true }));
    try {
      const updated = await api.updateChannel(channel.id, { enabled: !channel.enabled });
      setChannels((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setBusyIds((prev) => ({ ...prev, [channel.id]: false }));
    }
  };

  const startEdit = (channel: NotificationChannelView) => {
    setEditingId(channel.id);
    setEditName(channel.name);
    setEditTriggerDaily(channel.triggerOnDaily);
    setEditTriggerAlert(channel.triggerOnAlert);
    setEditUpdateConfig(false);
    setEditFields(emptyChannelFields());
  };

  const handleSaveEdit = async (channel: NotificationChannelView) => {
    setError(null);
    setStatusMessage(null);

    if (!editName.trim()) {
      setError(t('channels.nameEmptyError'));
      return;
    }

    const cmd: UpdateChannelCmd = {
      name: editName.trim(),
      triggerOnDaily: editTriggerDaily,
      triggerOnAlert: editTriggerAlert,
    };

    if (editUpdateConfig) {
      const { payload, validationError } = buildConfigPayload(channel.channelType, editFields, t);
      if (validationError || !payload) {
        setError(validationError ?? t('channels.invalidConfig'));
        return;
      }
      cmd.config = payload;
    }

    setSavingEdit(true);
    try {
      const updated = await api.updateChannel(channel.id, cmd);
      setChannels((prev) => prev.map((c) => (c.id === updated.id ? updated : c)));
      setEditingId(null);
      setStatusMessage(t('channels.updatedMsg', { name: updated.name }));
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setSavingEdit(false);
    }
  };

  const handleDelete = async (channel: NotificationChannelView) => {
    setError(null);
    setStatusMessage(null);
    setBusyIds((prev) => ({ ...prev, [channel.id]: true }));
    try {
      await api.deleteChannel(channel.id);
      setChannels((prev) => prev.filter((c) => c.id !== channel.id));
      if (editingId === channel.id) {
        setEditingId(null);
      }
      setStatusMessage(t('channels.deletedMsg', { name: channel.name }));
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setBusyIds((prev) => ({ ...prev, [channel.id]: false }));
    }
  };

  const handleTestChannel = async (channel: NotificationChannelView) => {
    setError(null);
    setBusyIds((prev) => ({ ...prev, [channel.id]: true }));
    try {
      const result = await api.testChannel(channel.id);
      setTestResults((prev) => ({ ...prev, [channel.id]: result }));
    } catch (err) {
      setError(formatError(err, t));
    } finally {
      setBusyIds((prev) => ({ ...prev, [channel.id]: false }));
    }
  };

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

      {/* Add Notification Channel */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <div>
          <h2 className="text-base font-semibold text-white">{t('channels.addTitle')}</h2>
          <p className="mt-0.5 text-xs text-slate-400">{t('channels.addDesc')}</p>
        </div>

        <form onSubmit={(e) => void handleAddChannel(e)} className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-2">
            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">{t('channels.channelType')}</span>
              <select
                value={addType}
                onChange={(e) => {
                  setAddType(e.target.value as NotificationChannelType);
                  setAddFields(emptyChannelFields());
                }}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              >
                {CHANNEL_TYPE_VALUES.map((val) => (
                  <option key={val} value={val}>
                    {labelChannelType(val)}
                  </option>
                ))}
              </select>
            </label>

            <label className="block space-y-1.5 text-xs">
              <span className="font-medium text-slate-200">{t('channels.channelName')}</span>
              <input
                type="text"
                value={addName}
                onChange={(e) => setAddName(e.target.value)}
                required
                placeholder={t('channels.channelNamePlaceholder')}
                className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-white"
              />
            </label>
          </div>

          <div className="rounded-lg border border-slate-800 bg-slate-950/50 p-4">
            <DynamicChannelConfigFields
              channelType={addType}
              fields={addFields}
              onChange={(patch) => setAddFields((prev) => ({ ...prev, ...patch }))}
            />
          </div>

          <div className="flex flex-wrap items-center gap-5 text-xs text-slate-300">
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={addTriggerDaily}
                onChange={(e) => setAddTriggerDaily(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>{t('channels.triggerOnDaily')}</span>
            </label>
            <label className="inline-flex items-center gap-2">
              <input
                type="checkbox"
                checked={addTriggerAlert}
                onChange={(e) => setAddTriggerAlert(e.target.checked)}
                className="h-4 w-4 rounded border-slate-600 bg-slate-900"
              />
              <span>{t('channels.triggerOnAlert')}</span>
            </label>
          </div>

          <button
            type="submit"
            disabled={creating}
            className="inline-flex items-center gap-2 rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
          >
            <Plus className="h-4 w-4" />
            {creating ? t('channels.creatingBtn') : t('channels.addBtn')}
          </button>
        </form>
      </section>

      {/* Configured Channels List */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/70 p-5 space-y-4">
        <h2 className="text-base font-semibold text-white">
          {t('channels.listTitle', { count: channels.length })}
        </h2>

        {loading ? (
          <p className="py-4 text-sm text-slate-400">{t('channels.loading')}</p>
        ) : channels.length === 0 ? (
          <p className="py-4 text-sm text-slate-500">{t('channels.empty')}</p>
        ) : (
          <div className="space-y-3">
            {channels.map((ch) => {
              const isEditing = editingId === ch.id;
              const isBusy = Boolean(busyIds[ch.id]);
              const testRes = testResults[ch.id];

              return (
                <div
                  key={ch.id}
                  className="rounded-lg border border-slate-800 bg-slate-950/60 p-4 space-y-3"
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-sm font-semibold text-white">{ch.name}</span>
                        <span className="rounded border border-slate-700 bg-slate-800 px-2 py-0.5 font-mono text-[11px] text-slate-300">
                          {labelChannelType(ch.channelType)}
                        </span>
                        {ch.triggerOnDaily && (
                          <span className="rounded border border-indigo-500/30 bg-indigo-500/10 px-2 py-0.5 text-[11px] text-indigo-300">
                            {t('channels.dailyTag')}
                          </span>
                        )}
                        {ch.triggerOnAlert && (
                          <span className="rounded border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-[11px] text-amber-300">
                            {t('channels.alertTag')}
                          </span>
                        )}
                      </div>
                      <div className="text-xs text-slate-400">
                        {t('channels.configHint', { hint: ch.configHint })}
                      </div>
                    </div>

                    <div className="flex flex-wrap items-center gap-2">
                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleToggleEnabled(ch)}
                        className={`rounded-lg border px-2.5 py-1.5 text-xs font-medium transition disabled:opacity-50 ${
                          ch.enabled
                            ? 'border-emerald-500/40 bg-emerald-500/15 text-emerald-200 hover:bg-emerald-500/25'
                            : 'border-slate-700 bg-slate-800 text-slate-300 hover:bg-slate-700'
                        }`}
                      >
                        {ch.enabled ? t('common.enabled') : t('common.disabled')}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleTestChannel(ch)}
                        className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:opacity-50"
                      >
                        <Send className="h-3.5 w-3.5 text-cyan-400" />
                        {t('channels.testSend')}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => (isEditing ? setEditingId(null) : startEdit(ch))}
                        className="inline-flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:opacity-50"
                      >
                        <Edit3 className="h-3.5 w-3.5" />
                        {isEditing ? t('common.cancel') : t('common.edit')}
                      </button>

                      <button
                        type="button"
                        disabled={isBusy}
                        onClick={() => void handleDelete(ch)}
                        className="inline-flex items-center gap-1 rounded-lg border border-rose-500/40 bg-rose-500/10 px-2.5 py-1.5 text-xs font-medium text-rose-200 hover:bg-rose-500/20 disabled:opacity-50"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        {t('common.delete')}
                      </button>
                    </div>
                  </div>

                  {isEditing && (
                    <div className="space-y-3 rounded-lg border border-slate-800 bg-slate-900/70 p-4">
                      <div className="text-xs text-slate-400">
                        {t('channels.storedHint')}{' '}
                        <span className="font-mono text-slate-200">{ch.configHint}</span>
                      </div>

                      <label className="block space-y-1 text-xs">
                        <span className="font-medium text-slate-200">
                          {t('channels.channelName')}
                        </span>
                        <input
                          type="text"
                          value={editName}
                          onChange={(e) => setEditName(e.target.value)}
                          className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-sm text-white"
                        />
                      </label>

                      <div className="flex flex-wrap items-center gap-5 text-xs text-slate-300">
                        <label className="inline-flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={editTriggerDaily}
                            onChange={(e) => setEditTriggerDaily(e.target.checked)}
                            className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                          />
                          <span>{t('channels.triggerOnDaily')}</span>
                        </label>
                        <label className="inline-flex items-center gap-2">
                          <input
                            type="checkbox"
                            checked={editTriggerAlert}
                            onChange={(e) => setEditTriggerAlert(e.target.checked)}
                            className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                          />
                          <span>{t('channels.triggerOnAlert')}</span>
                        </label>
                      </div>

                      <label className="inline-flex items-center gap-2 text-xs text-amber-300">
                        <input
                          type="checkbox"
                          checked={editUpdateConfig}
                          onChange={(e) => setEditUpdateConfig(e.target.checked)}
                          className="h-4 w-4 rounded border-slate-600 bg-slate-900"
                        />
                        <span>{t('channels.updateConfig')}</span>
                      </label>

                      {editUpdateConfig && (
                        <div className="rounded border border-slate-800 bg-slate-950/60 p-3">
                          <DynamicChannelConfigFields
                            channelType={ch.channelType}
                            fields={editFields}
                            onChange={(patch) => setEditFields((prev) => ({ ...prev, ...patch }))}
                          />
                        </div>
                      )}

                      <div>
                        <button
                          type="button"
                          disabled={savingEdit}
                          onClick={() => void handleSaveEdit(ch)}
                          className="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
                        >
                          {savingEdit ? t('common.saving') : t('common.saveChanges')}
                        </button>
                      </div>
                    </div>
                  )}

                  {testRes && (
                    <div
                      role="status"
                      className={`rounded border px-3 py-2 text-xs ${
                        testRes.status === DeliveryStatus.SENT
                          ? 'border-emerald-500/40 bg-emerald-950/30 text-emerald-200'
                          : 'border-rose-500/40 bg-rose-950/30 text-rose-200'
                      }`}
                    >
                      <div className="font-medium">
                        {t('channels.testSendResult', {
                          status: labelDeliveryStatus(testRes.status),
                          channelType: testRes.channelType,
                          time: formatDateTime(testRes.sentAt),
                        })}
                      </div>
                      {testRes.errorMessage && (
                        <div className="mt-1 text-rose-300">{testRes.errorMessage}</div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </section>
    </div>
  );
}
