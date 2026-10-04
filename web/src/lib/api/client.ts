// Minimal GraphQL-over-HTTP client. The API is small and typed by hand in
// ./types.ts, so a general-purpose GraphQL client would add more than it saves.

const ENDPOINT: string = import.meta.env.VITE_CODEATLAS_API ?? '/graphql';

/** An error returned by the API, with its `extensions.code` when present. */
export class ApiError extends Error {
	constructor(
		message: string,
		/** e.g. `NOT_FOUND`, `BAD_USER_INPUT`, `OUTDATED_INDEX`, `NETWORK`. */
		readonly code: string
	) {
		super(message);
		this.name = 'ApiError';
	}
}

interface GraphQLResponse<T> {
	data?: T | null;
	errors?: { message: string; extensions?: { code?: string } }[];
}

export async function request<T>(
	query: string,
	variables: Record<string, unknown> = {},
	fetchImpl: typeof fetch = fetch
): Promise<T> {
	let response: Response;
	try {
		response = await fetchImpl(ENDPOINT, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ query, variables })
		});
	} catch {
		throw new ApiError(
			`Cannot reach the CodeAtlas API at ${ENDPOINT}. Is codeatlas-server running?`,
			'NETWORK'
		);
	}
	let body: GraphQLResponse<T>;
	try {
		body = (await response.json()) as GraphQLResponse<T>;
	} catch {
		throw new ApiError(`The API answered ${response.status} without a JSON body.`, 'NETWORK');
	}
	if (body.errors?.length) {
		const [first] = body.errors;
		throw new ApiError(first.message, first.extensions?.code ?? 'UNKNOWN');
	}
	if (!body.data) {
		throw new ApiError(`The API answered ${response.status} without data.`, 'UNKNOWN');
	}
	return body.data;
}

/** Human-readable explanation for an error, with a next step where useful. */
export function describeError(error: unknown): string {
	if (error instanceof ApiError) {
		if (error.code === 'OUTDATED_INDEX') {
			return `${error.message}. Re-index the repository from the repository menu.`;
		}
		return error.message;
	}
	return error instanceof Error ? error.message : String(error);
}
