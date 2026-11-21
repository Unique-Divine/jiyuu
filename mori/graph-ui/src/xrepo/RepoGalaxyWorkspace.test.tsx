// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it } from 'vitest';
import RepoGalaxyWorkspace from './RepoGalaxyWorkspace';
import type { RepoGalaxySceneProps } from './RepoGalaxyScene';
import { XrepoClient } from './xrepo-api';

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
    (globalThis as unknown as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });

const summary = {
    repos: [
        { name: 'web', indexed: true, nodes: 4, edges: 3, links_out: 2, links_in: 0 },
        { name: 'api', indexed: true, nodes: 6, edges: 5, links_out: 0, links_in: 3 },
        { name: 'bus', indexed: true, nodes: 2, edges: 1, links_out: 1, links_in: 0 },
    ],
    links: [
        { from: 'web', to: 'api', type: 'CROSS_GRAPHQL_CALLS', count: 2 },
        { from: 'bus', to: 'api', type: 'CROSS_CHANNEL', count: 1 },
    ],
    types: ['CROSS_GRAPHQL_CALLS', 'CROSS_CHANNEL'],
    total_links: 3,
    skipped: [],
};
const links = {
    total: 2, limit: 100, offset: 0,
    links: [1, 2].map(line => ({ type: 'CROSS_GRAPHQL_CALLS', to: 'api', via: 'Orders', method: '',
        target_function: 'orders', target_file: 'graph/schema.resolvers.go',
        source: { name: 'useOrders', qualified_name: `web.useOrders${line}`, label: 'Function', file_path: 'src/orders.ts', line } })),
};

/** A local server: answers the two routes from the data above and records each request. */
function server(requests: string[]): XrepoClient {
    return new XrepoClient(async input => {
        const url = String(input);
        requests.push(url);
        const body = url.startsWith('/api/xrepo/summary') ? summary : links;
        return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
    });
}

/** Stands in for the WebGL scene: lists what it was asked to draw. */
function ListScene({ model, onSelectArc }: RepoGalaxySceneProps) {
    return <ul data-testid="scene">{model.arcs.map(arc =>
        <li key={arc.id}><button type="button" data-scene-arc={arc.id} onClick={() => onSelectArc(arc)}>{arc.id}</button></li>)}</ul>;
}

const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });

it('draws one arc per aggregate and lists the links behind a clicked arc', async () => {
    const requests: string[] = [];
    await act(async () => root.render(<RepoGalaxyWorkspace active client={server(requests)} scene={ListScene} />));
    await settle();
    expect([...host.querySelectorAll('[data-scene-arc]')].map(node => node.getAttribute('data-scene-arc')))
        .toEqual(['web->api:CROSS_GRAPHQL_CALLS', 'bus->api:CROSS_CHANNEL']);
    expect(host.textContent).toContain('3 cross-repo links');

    await act(async () => host.querySelector<HTMLButtonElement>('[data-scene-arc="web->api:CROSS_GRAPHQL_CALLS"]')!.click());
    await settle();
    expect(requests.at(-1)).toBe('/api/xrepo/links?from=web&to=api&type=CROSS_GRAPHQL_CALLS&limit=100');
    const rows = [...host.querySelectorAll('[data-testid="repo-galaxy-details"] tbody tr')];
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('src/orders.ts:1');
    expect(rows[0].textContent).toContain('graph/schema.resolvers.go');
});

it('filters arcs by link kind and switches theme', async () => {
    await act(async () => root.render(<RepoGalaxyWorkspace active client={server([])} scene={ListScene} />));
    await settle();
    await act(async () => host.querySelector<HTMLButtonElement>('[data-type="CROSS_CHANNEL"]')!.click());
    expect(host.querySelectorAll('[data-scene-arc]')).toHaveLength(1);
    const light = [...host.querySelectorAll<HTMLButtonElement>('.repo-galaxy-theme button')].find(button => button.textContent === 'Light')!;
    await act(async () => light.click());
    expect(host.querySelector('[data-testid="repo-galaxy"]')!.getAttribute('data-theme')).toBe('light');
});

it('reads nothing while hidden', async () => {
    const requests: string[] = [];
    await act(async () => root.render(<RepoGalaxyWorkspace active={false} client={server(requests)} scene={ListScene} />));
    await settle();
    expect(requests).toEqual([]);
});

/** A server with 150 links behind web -> api, served in pages. */
function pagedServer(requests: string[]): XrepoClient {
    return new XrepoClient(async input => {
        const url = new URL(String(input), 'http://127.0.0.1');
        requests.push(`${url.pathname}${url.search}`);
        if (url.pathname === '/api/xrepo/summary') {
            return new Response(JSON.stringify({ ...summary, links: [{ from: 'web', to: 'api', type: 'CROSS_GRAPHQL_CALLS', count: 150 }] }));
        }
        const offset = Number(url.searchParams.get('offset') ?? 0), limit = Number(url.searchParams.get('limit'));
        const rows = Array.from({ length: Math.max(0, Math.min(limit, 150 - offset)) }, (_, i) => ({
            ...links.links[0], source: { ...links.links[0].source, qualified_name: `web.op${offset + i}`, line: offset + i + 1 } }));
        return new Response(JSON.stringify({ total: 150, limit, offset, links: rows }));
    });
}

it('pages an arc with Show more, appending from the loaded length', async () => {
    const requests: string[] = [];
    await act(async () => root.render(<RepoGalaxyWorkspace active client={pagedServer(requests)} scene={ListScene} />));
    await settle();
    await act(async () => host.querySelector<HTMLButtonElement>('[data-scene-arc]')!.click());
    await settle();
    const rows = () => host.querySelectorAll('[data-testid="repo-galaxy-details"] tbody tr');
    expect(rows()).toHaveLength(100);
    const more = [...host.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.startsWith('Show more'))!;
    expect(more.textContent).toContain('100 of 150');
    await act(async () => more.click());
    await settle();
    expect(requests.at(-1)).toContain('offset=100');
    expect(rows()).toHaveLength(150);
    expect(rows()[149].textContent).toContain('src/orders.ts:150');
    expect([...host.querySelectorAll('button')].some(button => button.textContent?.startsWith('Show more'))).toBe(false);
});

it('shows an alert when the summary or an arc fails to load', async () => {
    const client = new XrepoClient(async () =>
        new Response(JSON.stringify({ error: 'cannot read the project cache' }), { status: 500 }));
    await act(async () => root.render(<RepoGalaxyWorkspace active client={client} scene={ListScene} />));
    await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('cannot read the project cache');

    const working = new XrepoClient(async input => String(input).startsWith('/api/xrepo/summary')
        ? new Response(JSON.stringify(summary))
        : new Response(JSON.stringify({ error: 'cannot read links' }), { status: 500 }));
    await act(async () => root.render(<RepoGalaxyWorkspace active client={working} scene={ListScene} />));
    await settle();
    await act(async () => host.querySelector<HTMLButtonElement>('[data-scene-arc]')!.click());
    await settle();
    expect(host.querySelector('[data-testid="repo-galaxy-details"] [role="alert"]')?.textContent).toContain('cannot read links');
});

it('reads the summary once across hiding and showing, and again on Refresh', async () => {
    const requests: string[] = [];
    const client = server(requests);
    const summaries = () => requests.filter(url => url.startsWith('/api/xrepo/summary')).length;
    await act(async () => root.render(<RepoGalaxyWorkspace active client={client} scene={ListScene} />));
    await settle();
    await act(async () => root.render(<RepoGalaxyWorkspace active={false} client={client} scene={ListScene} />));
    await act(async () => root.render(<RepoGalaxyWorkspace active client={client} scene={ListScene} />));
    await settle();
    expect(summaries()).toBe(1);
    const refresh = [...host.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Refresh')!;
    await act(async () => refresh.click());
    await settle();
    expect(summaries()).toBe(2);
});
