import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  FolderZip24Regular,
  Delete24Regular,
  Checkmark24Regular,
  FolderOpen24Regular,
  CheckmarkCircle24Filled,
  ArrowSync24Regular,
} from '@fluentui/react-icons';
import { invoke } from '@tauri-apps/api/core';
import { Button, Spinner, ProgressBar } from '@fluentui/react-components';
import type { MoleResult } from '../types';
import './Purger.css';

interface PurgeItem {
  path: string;
  sizeMB: number;
  name: string;
  status?: 'success' | 'failed';
}

interface PurgeSummary {
  totalCount: number;
  totalSize: string;
  cleaned?: number;
  failed?: number;
}

const targets = [
  { id: 'node', label: 'node_modules', pattern: '**/node_modules', color: 'var(--color-accent)' },
  { id: 'gradle', label: '.gradle / build', pattern: '**/.gradle, **/build', color: 'var(--color-blue)' },
  { id: 'cargo', label: 'target (Rust)', pattern: '**/target', color: 'var(--color-amber)' },
  { id: 'dart', label: '.dart_tool / build', pattern: '**/.dart_tool', color: 'var(--color-cyan)' },
  { id: 'python', label: '__pycache__ / .venv', pattern: '**/__pycache__', color: 'var(--color-purple)' },
  { id: 'dotnet', label: 'bin / obj (.NET)', pattern: '**/bin, **/obj', color: 'var(--color-red)' },
];

function parseOutput(output: string): { items: PurgeItem[]; summary: PurgeSummary | null } {
  const lines = output.split('\n').map(l => l.trim()).filter(Boolean);
  const items: PurgeItem[] = [];
  let summary: PurgeSummary | null = null;

  for (const line of lines) {
    // Parse: "  35405.1 MB  D:\path\to\target"
    const itemMatch = line.match(/^\s*([\d.]+)\s*MB\s+(.+)$/);
    if (itemMatch) {
      const sizeMB = parseFloat(itemMatch[1]);
      const fullPath = itemMatch[2].trim();
      const name = fullPath.split('\\').pop() || fullPath;
      items.push({ path: fullPath, sizeMB, name });
      continue;
    }

    // Parse: "  ✓  35405.1 MB  D:\path" (clean success)
    const successMatch = line.match(/^\s*✓\s*([\d.]+)\s*MB\s+(.+)$/);
    if (successMatch) {
      const sizeMB = parseFloat(successMatch[1]);
      const fullPath = successMatch[2].trim();
      const name = fullPath.split('\\').pop() || fullPath;
      items.push({ path: fullPath, sizeMB, name, status: 'success' });
      continue;
    }

    // Parse: "  ✗ D:\path  (access denied)" (clean failed)
    const failMatch = line.match(/^\s*✗\s+(.+?)\s+\(access denied\)/);
    if (failMatch) {
      const fullPath = failMatch[1].trim();
      const name = fullPath.split('\\').pop() || fullPath;
      items.push({ path: fullPath, sizeMB: 0, name, status: 'failed' });
      continue;
    }

    // Parse summary: "找到 334 個產物，共 174.88 GB"
    const summaryMatch = line.match(/找到\s*(\d+)\s*個產物.*?(\d+\.?\d*)\s*GB/);
    if (summaryMatch) {
      summary = { totalCount: parseInt(summaryMatch[1]), totalSize: `${summaryMatch[2]} GB` };
      continue;
    }

    // Parse clean summary: "清理完成: 10 個成功, 2 個跳過, 釋放 5.5 GB"
    const cleanMatch = line.match(/清理完成.*?(\d+)\s*個成功.*?(\d+)\s*個跳過.*?(\d+\.?\d*)\s*GB/);
    if (cleanMatch) {
      summary = {
        totalCount: parseInt(cleanMatch[1]) + parseInt(cleanMatch[2]),
        totalSize: `${cleanMatch[3]} GB`,
        cleaned: parseInt(cleanMatch[1]),
        failed: parseInt(cleanMatch[2]),
      };
    }
  }

  return { items, summary };
}

function formatSize(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb.toFixed(1)} MB`;
}

export function Purger() {
  const { t } = useTranslation();
  const [running, setRunning] = useState(false);
  const [dryRun, setDryRun] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [items, setItems] = useState<PurgeItem[]>([]);
  const [summary, setSummary] = useState<PurgeSummary | null>(null);
  const [error, setError] = useState('');

  const handlePurge = async (preview: boolean) => {
    setRunning(true);
    setDryRun(preview);
    setScanned(false);
    setItems([]);
    setSummary(null);
    setError('');
    try {
      const res = await invoke<MoleResult>('mole_purge', { dryRun: preview });
      const parsed = parseOutput(res.stdout || res.stderr || '');
      setItems(parsed.items);
      setSummary(parsed.summary);
      setScanned(true);
    } catch (err) {
      setError(`${err}`);
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="page purger">
      <h2><FolderZip24Regular /> {t('purger.title')}</h2>
      <p className="page-desc">{t('purger.desc')}</p>

      <div className="purge-targets">
        {targets.map((tgt, i) => (
          <div key={tgt.id} className={`glass-card purge-card stagger-${i + 1}`}>
            <div className="purge-icon" style={{ color: tgt.color }}>
              <FolderOpen24Regular />
            </div>
            <div className="purge-info">
              <span className="purge-label">{tgt.label}</span>
              <span className="purge-pattern">{tgt.pattern}</span>
            </div>
            {scanned && <Checkmark24Regular className="purge-check" />}
          </div>
        ))}
      </div>

      {/* Action buttons — always visible above results */}
      <div className="purge-actions">
        <Button size="large" appearance="subtle" icon={<ArrowSync24Regular />} onClick={() => handlePurge(true)} disabled={running}>
          {running && dryRun ? t('purger.scanning') : t('purger.dryRun')}
        </Button>
        {scanned && (
          <Button size="large" appearance="primary" icon={running ? undefined : <Delete24Regular />} onClick={() => handlePurge(false)} disabled={running}>
            {running && !dryRun ? t('purger.cleaning') : t('purger.startClean')}
          </Button>
        )}
      </div>

      {/* Running indicator */}
      {running && (
        <div className="glass-card purge-progress">
          <div className="purge-progress-header">
            <Spinner size="tiny" />
            <span>{dryRun ? t('purger.scanning') : t('purger.cleaning')}</span>
          </div>
          <ProgressBar />
        </div>
      )}

      {/* Parsed results as GUI cards */}
      {items.length > 0 && (
        <div className="purge-results">
          {items.map((item, i) => (
            <div key={i} className={`purge-result-item ${item.status === 'failed' ? 'failed' : ''}`}>
              <div className="purge-result-icon">
                {item.status === 'success' ? (
                  <CheckmarkCircle24Filled className="icon-success" />
                ) : item.status === 'failed' ? (
                  <Delete24Regular className="icon-failed" />
                ) : (
                  <FolderOpen24Regular className="icon-folder" />
                )}
              </div>
              <div className="purge-result-info">
                <span className="purge-result-name">{item.name}</span>
                <span className="purge-result-path">{item.path}</span>
              </div>
              {item.sizeMB > 0 && (
                <span className={`purge-result-size ${item.sizeMB >= 1024 ? 'large' : item.sizeMB >= 100 ? 'medium' : ''}`}>
                  {formatSize(item.sizeMB)}
                </span>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Summary card */}
      {summary && (
        <div className="glass-card purge-summary">
          <div className="summary-grid">
            <div className="summary-stat main">
              <span className="summary-value">{summary.totalSize}</span>
              <span className="summary-label">{summary.cleaned != null ? t('purger.freedSpace') : t('purger.reclaimableSpace')}</span>
            </div>
            <div className="summary-stat">
              <span className="summary-value">{summary.totalCount}</span>
              <span className="summary-label">{t('purger.artifacts')}</span>
            </div>
            {summary.cleaned != null && (
              <div className="summary-stat">
                <span className="summary-value">{summary.cleaned}</span>
                <span className="summary-label">{t('purger.cleaned')}</span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Error display */}
      {error && (
        <div className="glass-card result-output error">
          <pre>{error}</pre>
        </div>
      )}
    </div>
  );
}
