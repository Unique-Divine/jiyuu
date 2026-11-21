import { describe, expect, it } from 'vitest';
import { arcPoints, arcWidth, buildGalaxyModel, ringOrder, sphereRadius, typeLabel } from './repo-galaxy-model';
import { readXrepoLinks, readXrepoSummary, xrepoLinksUrl } from './xrepo-api';

const summary = readXrepoSummary({
    repos: [
        { name: 'web', indexed: true, nodes: 40, edges: 90, links_out: 12, links_in: 0 },
        { name: 'api', indexed: true, nodes: 80, edges: 200, links_out: 0, links_in: 15 },
        { name: 'bus', indexed: true, nodes: 10, edges: 12, links_out: 3, links_in: 0 },
        { name: 'docs', indexed: true, nodes: 5, edges: 4, links_out: 0, links_in: 0 },
    ],
    links: [
        { from: 'web', to: 'api', type: 'CROSS_GRAPHQL_CALLS', count: 10 },
        { from: 'web', to: 'api', type: 'CROSS_HTTP_CALLS', count: 2 },
        { from: 'bus', to: 'api', type: 'CROSS_CHANNEL', count: 3 },
        { from: 'bus', to: 'gone', type: 'CROSS_HTTP_CALLS', count: 1 },
        { from: 'x', to: 'y', type: 'CROSS_HTTP_CALLS', count: 0 },
        'not a row',
    ],
    types: ['CROSS_GRAPHQL_CALLS', 'CROSS_HTTP_CALLS', 'CROSS_CHANNEL'],
    total_links: 16,
    skipped: [],
});

describe('readXrepoSummary', () => {
    it('keeps valid rows and drops malformed or empty ones', () => {
        expect(summary.repos.map(repo => repo.name)).toEqual(['web', 'api', 'bus', 'docs']);
        expect(summary.links).toHaveLength(4);
        expect(summary.totalLinks).toBe(16);
    });

    it('rejects a non-object body', () => {
        expect(() => readXrepoSummary([])).toThrow();
    });
});

describe('buildGalaxyModel', () => {
    it('places linked repos and a target without an index, one arc per aggregate', () => {
        const model = buildGalaxyModel(summary);
        expect(model.nodes.map(node => node.name).sort()).toEqual(['api', 'bus', 'gone', 'web']);
        expect(model.nodes.find(node => node.name === 'gone')?.indexed).toBe(false);
        expect(model.arcs.map(arc => arc.id).sort()).toEqual([
            'bus->api:CROSS_CHANNEL', 'bus->gone:CROSS_HTTP_CALLS',
            'web->api:CROSS_GRAPHQL_CALLS', 'web->api:CROSS_HTTP_CALLS',
        ]);
        const api = model.nodes.find(node => node.name === 'api')!;
        const bus = model.nodes.find(node => node.name === 'bus')!;
        expect(api.links).toBe(15);
        expect(api.radius).toBeGreaterThan(bus.radius);
        for (const node of model.nodes) expect(Math.hypot(...node.position)).toBeLessThanOrEqual(model.extent);
    });

    it('filters by type and drops repos left without links unless asked', () => {
        const graphql = buildGalaxyModel(summary, { types: new Set(['CROSS_GRAPHQL_CALLS']) });
        expect(graphql.nodes.map(node => node.name).sort()).toEqual(['api', 'web']);
        expect(graphql.arcs).toHaveLength(1);
        const all = buildGalaxyModel(summary, { types: new Set(['CROSS_GRAPHQL_CALLS']), showUnlinked: true });
        expect(all.nodes.map(node => node.name).sort()).toEqual(['api', 'bus', 'docs', 'web']);
    });

    it('separates arcs of one pair by type', () => {
        const model = buildGalaxyModel(summary);
        const [graphql, http] = ['CROSS_GRAPHQL_CALLS', 'CROSS_HTTP_CALLS']
            .map(type => model.arcs.find(arc => arc.id === `web->api:${type}`)!);
        const peak = (points: number[][]) => Math.max(...points.map(point => point[1]));
        expect(peak(http.points)).toBeGreaterThan(peak(graphql.points));
    });
});

describe('ringOrder', () => {
    it('keeps the two busiest repos off neighbouring slots and places every repo once', () => {
        for (const n of [3, 4, 5, 6, 9]) {
            const repos = Array.from({ length: n }, (_, i) => ({ name: `r${i}`, links: 100 - i }));
            const ring = ringOrder(repos);
            expect(new Set(ring.map(repo => repo.name)).size).toBe(n);
            const gap = Math.abs(ring.findIndex(r => r.name === 'r0') - ring.findIndex(r => r.name === 'r1'));
            if (n > 3) expect(Math.min(gap, n - gap)).toBeGreaterThan(1);
        }
    });
});

describe('scales and labels', () => {
    it('grows width and radius with count, within bounds', () => {
        expect(arcWidth(1)).toBeLessThan(arcWidth(100));
        expect(arcWidth(1e9)).toBe(7);
        expect(sphereRadius(0)).toBeLessThan(sphereRadius(50));
        expect(sphereRadius(1e9)).toBe(6);
    });

    it('starts and ends arcs on the repos they join', () => {
        const points = arcPoints([0, 0, 0], [10, 0, 0], 0);
        expect(points[0]).toEqual([0, 0, 0]);
        expect(points.at(-1)).toEqual([10, 0, 0]);
    });

    it('names link kinds for people', () => {
        expect(typeLabel('CROSS_GRAPHQL_CALLS')).toBe('GraphQL');
        expect(typeLabel('CROSS_CHANNEL')).toBe('Channel');
        expect(typeLabel('CROSS_PACKAGE_USES')).toBe('Package Uses');
    });
});

describe('links route', () => {
    it('builds the query and reads a page', () => {
        expect(xrepoLinksUrl({ from: 'web', to: 'api', type: 'CROSS_HTTP_CALLS', limit: 50, offset: 50 }))
            .toBe('/api/xrepo/links?from=web&to=api&type=CROSS_HTTP_CALLS&limit=50&offset=50');
        const page = readXrepoLinks({ total: 1, limit: 100, offset: 0, links: [{ type: 'CROSS_HTTP_CALLS', to: 'api', via: '/orders',
            source: { name: 'fetchOrders', file_path: 'src/client.ts', line: 12 } }] });
        expect(page.links[0].source).toMatchObject({ filePath: 'src/client.ts', line: 12 });
    });
});
