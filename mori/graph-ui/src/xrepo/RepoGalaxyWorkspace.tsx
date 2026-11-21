import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ComponentType } from 'react';
import { buildGalaxyModel, typeLabel, type GalaxyArc } from './repo-galaxy-model';
import { arcColor, type GalaxyTheme, type RepoGalaxySceneProps } from './RepoGalaxyScene';
import { XrepoClient, type XrepoLink, type XrepoSummary } from './xrepo-api';
import './repo-galaxy.css';

const RepoGalaxyScene = lazy(() => import('./RepoGalaxyScene'));

const PAGE = 100;
const THEME_KEY = 'cbm-repo-galaxy-theme';

function initialTheme(): GalaxyTheme {
    try {
        const saved = window.localStorage.getItem(THEME_KEY);
        if (saved === 'light' || saved === 'dark') return saved;
    } catch { /* storage may be blocked */ }
    return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

interface ArcLinks {
    arcId: string;
    links: XrepoLink[];
    total: number;
    loading: boolean;
    error?: string;
}

export interface RepoGalaxyWorkspaceProps {
    active: boolean;
    client?: XrepoClient;
    /** The 3D scene; replaceable where WebGL is unavailable (jsdom tests). */
    scene?: ComponentType<RepoGalaxySceneProps>;
}

/** The Repos workspace: every indexed repository and the links between them. */
export default function RepoGalaxyWorkspace({ active, client: given, scene: Scene = RepoGalaxyScene }: RepoGalaxyWorkspaceProps) {
    const client = useMemo(() => given ?? new XrepoClient(), [given]);
    const [summary, setSummary] = useState<XrepoSummary>();
    const [error, setError] = useState<string>();
    const [loading, setLoading] = useState(false);
    const [revision, setRevision] = useState(0);
    const [disabled, setDisabled] = useState<ReadonlySet<string>>(new Set());
    const [showUnlinked, setShowUnlinked] = useState(false);
    const [theme, setTheme] = useState<GalaxyTheme>(initialTheme);
    const [selectedArc, setSelectedArc] = useState<GalaxyArc>();
    const [selectedRepo, setSelectedRepo] = useState<string>();
    const [arcLinks, setArcLinks] = useState<ArcLinks>();

    // The summary reads every project index on the server, so it is fetched
    // once per mount (on first activation) and again only on Refresh; hiding
    // and showing the tab keeps the last answer.
    const loaded = useRef<{ client: XrepoClient; revision: number }>(undefined);
    useEffect(() => {
        if (!active || (loaded.current?.client === client && loaded.current.revision === revision)) return;
        const abort = new AbortController();
        setLoading(true);
        client.summary(abort.signal).then(value => {
            loaded.current = { client, revision };
            setSummary(value); setError(undefined); setLoading(false);
        }, (reason: unknown) => {
            if (abort.signal.aborted) return;
            loaded.current = { client, revision };
            setError(reason instanceof Error ? reason.message : String(reason)); setLoading(false);
        });
        return () => { abort.abort(); setLoading(false); };
    }, [active, client, revision]);

    const types = useMemo(() => summary?.types ?? [], [summary]);
    const enabled = useMemo(() => new Set(types.filter(type => !disabled.has(type))), [types, disabled]);
    const model = useMemo(() => summary ? buildGalaxyModel(summary, { types: enabled, showUnlinked }) : undefined,
        [summary, enabled, showUnlinked]);
    const countByType = useMemo(() => {
        const counts = new Map<string, number>();
        for (const link of summary?.links ?? []) counts.set(link.type, (counts.get(link.type) ?? 0) + link.count);
        return counts;
    }, [summary]);

    const loadLinks = useCallback((arc: GalaxyArc, offset: number) => {
        setArcLinks(current => ({ arcId: arc.id, total: current?.arcId === arc.id ? current.total : 0,
            links: current?.arcId === arc.id && offset > 0 ? current.links : [], loading: true }));
        client.links({ from: arc.from, to: arc.to, type: arc.type, limit: PAGE, offset }).then(page => {
            setArcLinks(current => current?.arcId !== arc.id ? current : {
                arcId: arc.id, total: page.total, loading: false,
                links: offset > 0 ? [...current.links, ...page.links] : page.links });
        }, (reason: unknown) => {
            setArcLinks(current => current?.arcId !== arc.id ? current : { ...current, loading: false,
                error: reason instanceof Error ? reason.message : String(reason) });
        });
    }, [client]);

    const selectArc = useCallback((arc: GalaxyArc) => {
        setSelectedArc(arc); setSelectedRepo(undefined); loadLinks(arc, 0);
    }, [loadLinks]);
    const clear = useCallback(() => { setSelectedArc(undefined); setSelectedRepo(undefined); setArcLinks(undefined); }, []);
    const chooseTheme = (next: GalaxyTheme) => {
        setTheme(next);
        try { window.localStorage.setItem(THEME_KEY, next); } catch { /* per-viewer convenience only */ }
    };
    const toggleType = (type: string) => setDisabled(current => {
        const next = new Set(current);
        if (next.has(type)) next.delete(type); else next.add(type);
        return next;
    });

    const visibleArcs = useMemo(() => [...(model?.arcs ?? [])]
        .filter(arc => !selectedRepo || arc.from === selectedRepo || arc.to === selectedRepo)
        .sort((a, b) => b.count - a.count), [model, selectedRepo]);

    return <section className="repo-galaxy" data-theme={theme} data-testid="repo-galaxy" aria-label="Repos">
        <header className="repo-galaxy-bar">
            <div className="repo-galaxy-title">
                <h2>Repos</h2>
                <p>{summary ? `${summary.repos.filter(repo => repo.indexed).length} indexed repositories · ${summary.totalLinks.toLocaleString()} cross-repo links` : loading ? 'Reading project indexes…' : ''}</p>
            </div>
            <div className="repo-galaxy-chips" role="group" aria-label="Link kinds">
                {types.map(type => <button key={type} type="button" className="repo-galaxy-chip" aria-pressed={enabled.has(type)}
                    data-type={type} onClick={() => toggleType(type)} style={{ ['--chip' as string]: arcColor(type, theme) }}>
                    <span aria-hidden="true" />{typeLabel(type)} <small>{(countByType.get(type) ?? 0).toLocaleString()}</small>
                </button>)}
            </div>
            <label className="repo-galaxy-toggle">
                <input type="checkbox" checked={showUnlinked} onChange={event => setShowUnlinked(event.target.checked)} />
                Unlinked repos
            </label>
            <div className="repo-galaxy-theme" role="group" aria-label="Theme">
                {(['light', 'dark'] as const).map(value => <button key={value} type="button" aria-pressed={theme === value}
                    onClick={() => chooseTheme(value)}>{value === 'light' ? 'Light' : 'Dark'}</button>)}
            </div>
            <button type="button" onClick={() => setRevision(value => value + 1)} disabled={loading}>Refresh</button>
        </header>
        <div className="repo-galaxy-body">
            <div className="repo-galaxy-stage">
                {error && <p className="repo-galaxy-note" role="alert">Could not read cross-repo links: {error}</p>}
                {!error && summary && model && !model.arcs.length && <p className="repo-galaxy-note">
                    No cross-repo links stored for the selected kinds. Links appear after a cross-repo run
                    (index_repository with mode cross-repo-intelligence).</p>}
                {model && active && <Suspense fallback={null}>
                    <Scene model={model} theme={theme} active={active}
                        selectedArc={selectedArc?.id} selectedRepo={selectedRepo}
                        onSelectArc={selectArc}
                        onSelectRepo={name => { setSelectedRepo(current => current === name ? undefined : name); setSelectedArc(undefined); setArcLinks(undefined); }}
                        onClear={clear} />
                </Suspense>}
            </div>
            <aside className="repo-galaxy-panel" aria-label="Link details">
                {selectedArc ? <ArcDetails arc={selectedArc} theme={theme} state={arcLinks}
                    onMore={() => loadLinks(selectedArc, arcLinks?.links.length ?? 0)} onBack={clear} />
                    : <>
                        <h3>{selectedRepo ? `Links of ${selectedRepo}` : 'All links'}</h3>
                        {selectedRepo && <button type="button" className="repo-galaxy-link" onClick={clear}>Show all</button>}
                        <ol className="repo-galaxy-arcs">
                            {visibleArcs.map(arc => <li key={arc.id}>
                                <button type="button" onClick={() => selectArc(arc)} data-arc={arc.id}>
                                    <span className="repo-galaxy-swatch" style={{ background: arcColor(arc.type, theme) }} aria-hidden="true" />
                                    <span>{arc.from} → {arc.to}</span>
                                    <small>{typeLabel(arc.type)} · {arc.count.toLocaleString()}</small>
                                </button>
                            </li>)}
                        </ol>
                        {summary?.skipped.length ? <p className="repo-galaxy-note">Unreadable indexes: {summary.skipped.join(', ')}</p> : null}
                        {summary?.truncated ? <p className="repo-galaxy-note">The cache holds more projects than one summary reads; later names are left out.</p> : null}
                    </>}
            </aside>
        </div>
    </section>;
}

function ArcDetails({ arc, theme, state, onMore, onBack }: {
    arc: GalaxyArc; theme: GalaxyTheme; state?: ArcLinks; onMore: () => void; onBack: () => void;
}) {
    const rows = state?.arcId === arc.id ? state.links : [];
    return <div className="repo-galaxy-details" data-testid="repo-galaxy-details">
        <button type="button" className="repo-galaxy-link" onClick={onBack}>← All links</button>
        <h3><span className="repo-galaxy-swatch" style={{ background: arcColor(arc.type, theme) }} aria-hidden="true" />
            {arc.from} → {arc.to}</h3>
        <p>{typeLabel(arc.type)} · {arc.count.toLocaleString()} links</p>
        {state?.error && <p className="repo-galaxy-note" role="alert">{state.error}</p>}
        <table>
            <thead><tr><th>Source</th><th>Via</th><th>Target</th></tr></thead>
            <tbody>
                {rows.map((link, index) => <tr key={`${link.source.qualifiedName}:${link.via}:${index}`}>
                    <td><code>{link.source.filePath || '(no file)'}{link.source.line ? `:${link.source.line}` : ''}</code>
                        <small>{link.source.name}</small></td>
                    <td>{link.method ? `${link.method} ` : ''}{link.via}</td>
                    <td>{link.targetFunction || '(unnamed)'}<small>{link.targetFile}</small></td>
                </tr>)}
            </tbody>
        </table>
        {state?.loading && <p className="repo-galaxy-note">Loading links…</p>}
        {state && !state.loading && rows.length < state.total && <button type="button" onClick={onMore}>
            Show more ({rows.length} of {state.total})</button>}
    </div>;
}
