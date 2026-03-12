import { useState, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Button,
  Spinner,
} from '@fluentui/react-components';
import {
  Globe24Regular,
  FolderOpen24Regular,
  Document24Regular,
  Apps24Regular,
  Delete24Regular,
  Checkmark24Regular,
  CheckmarkCircle24Filled,
} from '@fluentui/react-icons';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { MoleResult } from '../types';
import './Cleaner.css';

interface ScanItem {
  name: string;
  size: string;
  count?: number;
}

interface ScanSection {
  name: string;
  items: ScanItem[];
  done: boolean;
}

type ScanEvent =
  | { type: 'section'; name: string }
  | { type: 'item'; name: string; size: string; count?: number }
  | { type: 'success'; name: string }
  | { type: 'info'; text: string }
  | { type: 'done'; success: boolean };

export function Cleaner() {
  const { t } = useTranslation();
  const [scanning, setScanning] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [scanned, setScanned] = useState(false);
  const [sections, setSections] = useState<ScanSection[]>([]);
  const [infoLines, setInfoLines] = useState<string[]>([]);
  const [scanDone, setScanDone] = useState(false);
  const [error, setError] = useState('');

  const categories = [
    { id: 'browser', label: t('cleaner.browserCache'), icon: <Globe24Regular />, color: 'var(--color-blue)', desc: t('cleaner.browserCacheDesc') },
    { id: 'temp', label: t('cleaner.systemTemp'), icon: <FolderOpen24Regular />, color: 'var(--color-amber)', desc: t('cleaner.systemTempDesc') },
    { id: 'logs', label: t('cleaner.logs'), icon: <Document24Regular />, color: 'var(--color-purple)', desc: t('cleaner.logsDesc') },
    { id: 'apps', label: t('cleaner.appLeftovers'), icon: <Apps24Regular />, color: 'var(--color-cyan)', desc: t('cleaner.appLeftoversDesc') },
  ];

  // Calculate total size from all items
  const getTotalStats = useCallback(() => {
    let totalItems = 0;
    let totalSections = sections.length;
    sections.forEach(s => {
      totalItems += s.items.length;
    });
    // Try to find summary info from infoLines
    const sizeLine = infoLines.find(l => l.includes('Potential space:') || l.includes('Space freed:'));
    const totalSize = sizeLine
      ? sizeLine.replace(/.*:\s*/, '').replace(/[💾⚙📦📂✅\s]/g, '').trim()
      : '';
    return { totalItems, totalSections, totalSize };
  }, [sections, infoLines]);

  const handleScan = async () => {
    setScanning(true);
    setSections([]);
    setInfoLines([]);
    setScanDone(false);
    setError('');
    setScanned(false);

    try {
      await invoke<MoleResult>('mole_clean_streaming', { dryRun: true });
      setScanned(true);
    } catch (err) {
      setError(`${err}`);
    } finally {
      setScanning(false);
    }
  };

  const handleClean = async () => {
    setCleaning(true);
    setSections([]);
    setInfoLines([]);
    setScanDone(false);
    setError('');

    try {
      await invoke<MoleResult>('mole_clean_streaming', { dryRun: false });
    } catch (err) {
      setError(`${err}`);
    } finally {
      setCleaning(false);
    }
  };

  // Listen for scan-progress events
  useEffect(() => {
    const unlisten = listen<ScanEvent>('scan-progress', (event) => {
      const data = event.payload;

      switch (data.type) {
        case 'section':
          setSections(prev => {
            // Close previous section
            const updated = prev.map((s, i) =>
              i === prev.length - 1 ? { ...s, done: true } : s
            );
            return [...updated, { name: data.name, items: [], done: false }];
          });
          break;

        case 'item':
          setSections(prev => {
            if (prev.length === 0) return prev;
            const last = prev[prev.length - 1];
            return [
              ...prev.slice(0, -1),
              { ...last, items: [...last.items, { name: data.name, size: data.size, count: data.count ?? undefined }] }
            ];
          });
          break;

        case 'success':
          setSections(prev => {
            if (prev.length === 0) return prev;
            const last = prev[prev.length - 1];
            return [
              ...prev.slice(0, -1),
              { ...last, items: [...last.items, { name: data.name, size: '' }] }
            ];
          });
          break;

        case 'info':
          setInfoLines(prev => [...prev, data.text]);
          break;

        case 'done':
          setSections(prev => prev.map(s => ({ ...s, done: true })));
          setScanDone(true);
          break;
      }
    });

    return () => { unlisten.then(fn => fn()); };
  }, []);

  const stats = getTotalStats();

  return (
    <div className="page cleaner">
      <div className="page-header">
        <h2><Delete24Regular /> {t('cleaner.title')}</h2>
        <p className="page-desc">{t('cleaner.desc')}</p>
      </div>

      {/* Category cards */}
      <div className="cleaner-categories">
        {categories.map((cat, i) => (
          <div key={cat.id} className={`glass-card category-card stagger-${i + 1}`}>
            <div className="category-icon" style={{ color: cat.color }}>{cat.icon}</div>
            <div className="category-info">
              <span className="category-label">{cat.label}</span>
              <span className="category-desc">{cat.desc}</span>
            </div>
          </div>
        ))}
      </div>

      {/* Action buttons — always visible */}
      <div className="cleaner-actions">
        <Button appearance="primary" size="large" icon={scanning ? undefined : <Delete24Regular />} onClick={handleScan} disabled={scanning || cleaning}>
          {scanning ? t('cleaner.scanning') : t('cleaner.scan')}
        </Button>
        {scanned && (
          <Button appearance="primary" size="large" icon={cleaning ? undefined : <Checkmark24Regular />} onClick={handleClean} disabled={scanning || cleaning}>
            {cleaning ? t('cleaner.cleaning') : t('cleaner.clean')}
          </Button>
        )}
      </div>

      {/* Streaming scan results */}
      {(sections.length > 0 || scanning || cleaning) && (
        <div className="scan-results">
          {sections.map((section, si) => (
            <div key={si} className={`glass-card scan-section ${section.done ? 'done' : 'active'}`}>
              <div className="scan-section-header">
                <span className="scan-section-name">
                  {section.done
                    ? <CheckmarkCircle24Filled className="section-icon done" />
                    : <Spinner size="tiny" className="section-icon" />
                  }
                  {section.name}
                </span>
                {section.items.length > 0 && (
                  <span className="scan-section-count">{t('cleaner.sectionItems', { count: section.items.length })}</span>
                )}
              </div>
              {section.items.length > 0 && (
                <div className="scan-items">
                  {section.items.map((item, ii) => (
                    <div key={ii} className="scan-item">
                      <span className="scan-item-name">{item.name}</span>
                      <div className="scan-item-meta">
                        {item.count && <span className="scan-item-count">{item.count} 項</span>}
                        {item.size && <span className="scan-item-size">{item.size}</span>}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ))}

          {/* Active scanning indicator */}
          {(scanning || cleaning) && !scanDone && (
            <div className="glass-card scan-section active scanning-pulse">
              <div className="scan-section-header">
                <span className="scan-section-name">
                  <Spinner size="tiny" className="section-icon" />
                  {scanning ? t('cleaner.scanningProgress') : t('cleaner.cleaningProgress')}
                </span>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Summary card */}
      {scanDone && stats.totalSize && (
        <div className="glass-card scan-summary">
          <div className="summary-grid">
            <div className="summary-stat main">
              <span className="summary-value">{stats.totalSize}</span>
              <span className="summary-label">{cleaning ? t('cleaner.freedSpace') : t('cleaner.reclaimableSpace')}</span>
            </div>
            <div className="summary-stat">
              <span className="summary-value">{stats.totalItems}</span>
              <span className="summary-label">{t('cleaner.items')}</span>
            </div>
            <div className="summary-stat">
              <span className="summary-value">{stats.totalSections}</span>
              <span className="summary-label">{t('cleaner.categories')}</span>
            </div>
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
