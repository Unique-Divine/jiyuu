/**
 * Cross-repo routes: `GET /api/xrepo/summary` (repositories and links
 * aggregated per from, to and type) and `GET /api/xrepo/links` (the links
 * behind one aggregate, paged). Both read the CROSS_* edges the cross-repo
 * pass stored, across every project index the server can see.
 */

export interface XrepoRepo {
    name: string;
    /** False for a link target that has no index of its own. */
    indexed: boolean;
    nodes: number;
    edges: number;
    linksOut: number;
    linksIn: number;
}

export interface XrepoAggregate {
    from: string;
    to: string;
    type: string;
    count: number;
}

export interface XrepoSummary {
    repos: XrepoRepo[];
    links: XrepoAggregate[];
    types: string[];
    totalLinks: number;
    skipped: string[];
    /** The server read only the first projects by name. */
    truncated: boolean;
}

export interface XrepoLink {
    type: string;
    to: string;
    /** What the link goes through: a GraphQL operation, URL path, channel... */
    via: string;
    method: string;
    targetFunction: string;
    targetFile: string;
    source: { name: string; qualifiedName: string; label: string; filePath: string; line: number };
}

export interface XrepoLinksPage {
    total: number;
    limit: number;
    offset: number;
    links: XrepoLink[];
}

export interface XrepoLinksQuery {
    from: string;
    to?: string;
    type?: string;
    limit?: number;
    offset?: number;
}

type Json = Record<string, unknown>;

const isRecord = (value: unknown): value is Json =>
    typeof value === 'object' && value !== null && !Array.isArray(value);
const str = (value: unknown): string => (typeof value === 'string' ? value : '');
const num = (value: unknown): number => (typeof value === 'number' && Number.isFinite(value) ? value : 0);
const list = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);

/** Validate a summary response; malformed rows are dropped, not trusted. */
export function readXrepoSummary(value: unknown): XrepoSummary {
    if (!isRecord(value)) throw new Error('cross-repo summary is not an object');
    const repos = list(value.repos).filter(isRecord).map(row => ({
        name: str(row.name),
        indexed: row.indexed === true,
        nodes: num(row.nodes),
        edges: num(row.edges),
        linksOut: num(row.links_out),
        linksIn: num(row.links_in),
    })).filter(repo => repo.name);
    const links = list(value.links).filter(isRecord).map(row => ({
        from: str(row.from), to: str(row.to), type: str(row.type), count: num(row.count),
    })).filter(link => link.from && link.to && link.type && link.count > 0);
    return {
        repos,
        links,
        types: list(value.types).map(str).filter(Boolean),
        totalLinks: num(value.total_links),
        skipped: list(value.skipped).map(str).filter(Boolean),
        truncated: value.truncated === true,
    };
}

export function readXrepoLinks(value: unknown): XrepoLinksPage {
    if (!isRecord(value)) throw new Error('cross-repo links response is not an object');
    return {
        total: num(value.total),
        limit: num(value.limit),
        offset: num(value.offset),
        links: list(value.links).filter(isRecord).map(row => {
            const source = isRecord(row.source) ? row.source : {};
            return {
                type: str(row.type),
                to: str(row.to),
                via: str(row.via),
                method: str(row.method),
                targetFunction: str(row.target_function),
                targetFile: str(row.target_file),
                source: {
                    name: str(source.name),
                    qualifiedName: str(source.qualified_name),
                    label: str(source.label),
                    filePath: str(source.file_path),
                    line: num(source.line),
                },
            };
        }),
    };
}

export function xrepoLinksUrl(query: XrepoLinksQuery): string {
    const params = new URLSearchParams({ from: query.from });
    if (query.to) params.set('to', query.to);
    if (query.type) params.set('type', query.type);
    if (query.limit) params.set('limit', String(query.limit));
    if (query.offset) params.set('offset', String(query.offset));
    return `/api/xrepo/links?${params.toString()}`;
}

export class XrepoClient {
    constructor(
        private readonly fetcher: typeof globalThis.fetch = (...args) => globalThis.fetch(...args),
        private readonly base = '',
    ) {}

    private async getJson(route: string, signal?: AbortSignal): Promise<unknown> {
        const response = await this.fetcher(`${this.base}${route}`, { signal, headers: { Accept: 'application/json' } });
        if (!response.ok) {
            let message = `${route} answered ${response.status}`;
            try {
                const body: unknown = await response.json();
                if (isRecord(body) && typeof body.error === 'string') message = `${message}: ${body.error}`;
            } catch { /* keep the status message */ }
            throw new Error(message);
        }
        return response.json();
    }

    async summary(signal?: AbortSignal): Promise<XrepoSummary> {
        return readXrepoSummary(await this.getJson('/api/xrepo/summary', signal));
    }

    async links(query: XrepoLinksQuery, signal?: AbortSignal): Promise<XrepoLinksPage> {
        return readXrepoLinks(await this.getJson(xrepoLinksUrl(query), signal));
    }
}
