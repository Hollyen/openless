// 服务 → OCR 设置：开启屏幕上下文、选择 OCR 提供方、管理模型下载。

import { useEffect, useState, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { Btn, Card } from '../_atoms';
import { Icon } from '../../components/Icon';
import { SelectLite } from '../../components/ui/SelectLite';
import {
  cancelOcrModelDownload,
  deleteOcrModels,
  getOcrDownloadStatus,
  getOcrSettings,
  openSystemSettings,
  setOcrSettings,
  startOcrModelDownload,
  testOcrRecognition,
  OCR_ERROR_SCREEN_RECORDING_DENIED,
  OCR_ERROR_WINRT_LANGUAGE_MISSING,
  type OcrProvider,
  type OcrSettings,
} from '../../lib/ipc';
import { SettingRow, SectionTitle, Toggle, inputStyle } from './shared';

/** 测试识别失败时的分类结果，用于设置页展示针对性的权限/语言包引导。 */
type OcrTestErrorKind = 'screenRecordingDenied' | 'winrtLanguageMissing' | 'other';

interface OcrTestError {
  kind: OcrTestErrorKind;
  /** 后端原始错误消息。 */
  raw: string;
}

/** 解析 `test_ocr_recognition` 抛出的错误，识别已知的权限/语言包标记。 */
function parseOcrTestError(message: string): OcrTestError {
  if (message.startsWith(OCR_ERROR_SCREEN_RECORDING_DENIED)) {
    return { kind: 'screenRecordingDenied', raw: message };
  }
  if (message.startsWith(OCR_ERROR_WINRT_LANGUAGE_MISSING)) {
    return { kind: 'winrtLanguageMissing', raw: message };
  }
  return { kind: 'other', raw: message };
}

function formatBytes(value?: number): string {
  if (value === undefined || value === null || value === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let v = value;
  let unit = 0;
  while (v >= 1024 && unit < units.length - 1) {
    v /= 1024;
    unit += 1;
  }
  return `${v.toFixed(1)} ${units[unit]}`;
}

function DownloadStatusList({ statuses }: { statuses: Awaited<ReturnType<typeof getOcrDownloadStatus>> }) {
  const { t } = useTranslation();
  if (statuses.length === 0) return null;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginTop: 8 }}>
      {statuses.map((s) => {
        const progress = s.bytesTotal && s.bytesTotal > 0
          ? Math.round((s.bytesDownloaded ?? 0) / s.bytesTotal * 100)
          : undefined;
        return (
          <div key={s.modelId} style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.5 }}>
            {s.modelId}: {t(`settings.ocr.state.${s.state}`)} {progress !== undefined ? `(${progress}%)` : ''}
            {s.error && <span style={{ color: 'var(--ol-warn)' }}> — {s.error}</span>}
          </div>
        );
      })}
    </div>
  );
}

export function OcrSection() {
  const { t } = useTranslation();
  const [settings, setSettingsState] = useState<OcrSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [downloadStatuses, setDownloadStatuses] = useState<Awaited<ReturnType<typeof getOcrDownloadStatus>>>([]);
  const [testResult, setTestResult] = useState<string | null>(null);
  const [testError, setTestError] = useState<OcrTestError | null>(null);
  const [testBusy, setTestBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const s = await getOcrSettings();
      setSettingsState(s);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 每 2 秒刷新下载进度。
  useEffect(() => {
    if (!settings?.requiresDownload) return;
    let cancelled = false;
    const refresh = async () => {
      try {
        const statuses = await getOcrDownloadStatus();
        if (!cancelled) setDownloadStatuses(statuses);
      } catch (err) {
        console.error('[ocr] failed to refresh download status', err);
      }
    };
    void refresh();
    const timer = window.setInterval(refresh, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [settings?.requiresDownload]);

  const save = async (next: Partial<OcrSettings>) => {
    if (!settings || saving) return;
    setSaving(true);
    try {
      await setOcrSettings({
        screenContextEnabled: next.screenContextEnabled ?? settings.screenContextEnabled,
        activeProviderId: next.activeProviderId ?? settings.activeProviderId,
        modelsBaseDir: next.modelsBaseDir ?? settings.modelsBaseDir,
        downloadMirror: next.downloadMirror ?? settings.downloadMirror,
      });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const onToggle = () => {
    if (!settings) return;
    void save({ screenContextEnabled: !settings.screenContextEnabled });
  };

  const onProviderChange = (id: string) => {
    if (!settings) return;
    void save({ activeProviderId: id });
  };

  const activeProvider = settings?.providers.find(p => p.id === settings.activeProviderId);
  const providerOptions = (settings?.providers ?? []).map(p => ({
    value: p.id,
    label: p.name,
  }));

  return (
    <Card>
      <div style={{ marginBottom: 10 }}>
        <SectionTitle>{t('settings.ocr.title')}</SectionTitle>
      </div>
      <SettingRow
        label={t('settings.ocr.enableLabel')}
        desc={t('settings.ocr.enableHint')}
      >
        <Toggle on={settings?.screenContextEnabled ?? false} onToggle={onToggle} />
      </SettingRow>
      {loading && (
        <div style={{ fontSize: 12, color: 'var(--ol-ink-4)', marginTop: 8 }}>
          {t('common.loading')}
        </div>
      )}
      {error && (
        <div style={{ fontSize: 11.5, color: 'var(--ol-warn)', lineHeight: 1.5, marginTop: 8 }}>
          {error}
        </div>
      )}
      {!loading && !error && settings && (
        <>
          <SettingRow label={t('settings.ocr.providerLabel')}>
            <SelectLite
              value={settings.activeProviderId}
              onChange={onProviderChange}
              options={providerOptions}
              ariaLabel={t('settings.ocr.providerLabel')}
              style={inputStyle}
            />
          </SettingRow>
          {activeProvider && (
            <div style={{ fontSize: 11.5, color: 'var(--ol-ink-4)', lineHeight: 1.6, marginTop: 2 }}>
              {activeProvider.description}
              {activeProvider.id === 'winrt' && (
                <>
                  {' '}
                  <span style={{ color: 'var(--ol-warn)' }}>
                    {t('settings.ocr.winrtLanguagePackHint')}
                  </span>
                </>
              )}
              <br />
              {t('settings.ocr.platforms')}: {activeProvider.supportedPlatforms.join(', ')}
            </div>
          )}
          {settings.activeProviderId === 'rapidocr' && (
            <>
              <SettingRow label={t('settings.ocr.modelsBaseDirLabel')}>
                <input
                  type="text"
                  value={settings.modelsBaseDir}
                  placeholder={settings.modelsRootDir}
                  onChange={(e) => setSettingsState({ ...settings, modelsBaseDir: e.target.value })}
                  onBlur={() => void save({ modelsBaseDir: settings.modelsBaseDir })}
                  style={inputStyle}
                />
              </SettingRow>
              <SettingRow label={t('settings.ocr.downloadMirrorLabel')} desc={t('settings.ocr.downloadMirrorHint')}>
                <input
                  type="text"
                  value={settings.downloadMirror}
                  placeholder="https://github.com/RapidAI/RapidOCR/releases/download/v2.0.0"
                  onChange={(e) => setSettingsState({ ...settings, downloadMirror: e.target.value })}
                  onBlur={() => void save({ downloadMirror: settings.downloadMirror })}
                  style={inputStyle}
                />
              </SettingRow>
              <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 8 }}>
                <Btn
                  variant="blue"
                  icon="download"
                  onClick={() => void startOcrModelDownload(settings.downloadMirror || undefined)}
                  disabled={saving}
                >
                  {t('settings.ocr.downloadModel')}
                </Btn>
                <Btn
                  variant="ghost"
                  icon="close"
                  onClick={() => void cancelOcrModelDownload()}
                  disabled={saving}
                >
                  {t('settings.ocr.cancelDownload')}
                </Btn>
                <Btn
                  variant="ghost"
                  icon="trash"
                  onClick={() => void deleteOcrModels()}
                  disabled={saving}
                >
                  {t('settings.ocr.deleteModel')}
                </Btn>
              </div>
              <DownloadStatusList statuses={downloadStatuses} />
              {downloadStatuses.length > 0 && downloadStatuses.every(s => s.state === 'completed') && (
                <div style={{ fontSize: 11, color: 'var(--ol-ok)', marginTop: 8 }}>
                  {t('settings.ocr.downloadCompleted')}
                </div>
              )}
            </>
          )}
          <div style={{ display: 'flex', gap: 8, marginTop: 12 }}>
            <Btn
              variant="soft"
              icon="eye"
              onClick={async () => {
                setTestBusy(true);
                setTestResult(null);
                setTestError(null);
                try {
                  const text = await testOcrRecognition();
                  setTestResult(text ?? t('settings.ocr.noTextRecognized'));
                } catch (err) {
                  setTestError(parseOcrTestError(err instanceof Error ? err.message : String(err)));
                } finally {
                  setTestBusy(false);
                }
              }}
              disabled={testBusy || !settings.screenContextEnabled}
            >
              {testBusy ? t('common.loading') : t('settings.ocr.testRecognition')}
            </Btn>
          </div>
          {testResult !== null && (
            <div style={{ marginTop: 8 }}>
              <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginBottom: 4 }}>
                {t('settings.ocr.testResult')}:
              </div>
              <pre
                style={{
                  fontSize: 11,
                  lineHeight: 1.5,
                  maxHeight: 240,
                  overflow: 'auto',
                  padding: 10,
                  borderRadius: 8,
                  background: 'var(--ol-surface-2)',
                  color: 'var(--ol-ink)',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-word',
                }}
              >
                {testResult}
              </pre>
            </div>
          )}
          {testError !== null && (
            <div style={{ marginTop: 8, display: 'flex', flexDirection: 'column', gap: 8, alignItems: 'flex-start' }}>
              {testError.kind === 'screenRecordingDenied' && (
                <>
                  <div style={{ fontSize: 11.5, color: 'var(--ol-warn)', lineHeight: 1.5 }}>
                    {t('settings.ocr.screenRecordingDenied')}
                  </div>
                  <Btn
                    variant="soft"
                    icon="external"
                    onClick={() =>
                      void openSystemSettings('screen-recording').catch((err) =>
                        setError(err instanceof Error ? err.message : String(err)),
                      )
                    }
                  >
                    {t('settings.permissions.openSystem')}
                  </Btn>
                </>
              )}
              {testError.kind === 'winrtLanguageMissing' && (
                <div style={{ fontSize: 11.5, color: 'var(--ol-warn)', lineHeight: 1.5 }}>
                  {t('settings.ocr.winrtLanguageMissing')}
                </div>
              )}
              {testError.kind === 'other' && (
                <>
                  <div style={{ fontSize: 11, color: 'var(--ol-ink-4)', marginBottom: 4 }}>
                    {t('settings.ocr.testResult')}:
                  </div>
                  <pre
                    style={{
                      fontSize: 11,
                      lineHeight: 1.5,
                      maxHeight: 240,
                      overflow: 'auto',
                      padding: 10,
                      borderRadius: 8,
                      background: 'var(--ol-surface-2)',
                      color: 'var(--ol-warn)',
                      whiteSpace: 'pre-wrap',
                      wordBreak: 'break-word',
                    }}
                  >
                    {testError.raw}
                  </pre>
                </>
              )}
            </div>
          )}
        </>
      )}
    </Card>
  );
}
