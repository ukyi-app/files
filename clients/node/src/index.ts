/** 키의 커밋 포인터 메타. 서버 응답(camelCase)과 동일. */
export interface ObjectMeta {
  contentType: string;
  size: number;
  sha256: string;
  createdAt: string;
  uploadedBy: string;
}

export interface ObjectEntry extends ObjectMeta {
  key: string;
}

export type Visibility = "public" | "internal";

export interface BucketMeta {
  visibility: Visibility;
  owner: string;
  createdAt: string;
}

export interface BucketEntry extends BucketMeta {
  bucket: string;
}

export type FetchLike = typeof fetch;

export interface FilesClientOptions {
  baseUrl: string;
  apiKey: string;
  /** 주입형 fetch(테스트·커스텀 런타임). 기본 globalThis.fetch. */
  fetch?: FetchLike;
}

/** 비-2xx 응답에 대한 에러. `code`는 서버 `{"error": ...}` 본문. */
export class FilesError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string) {
    super(`files error ${status}: ${code}`);
    this.name = "FilesError";
    this.status = status;
    this.code = code;
  }
}

function encodePath(segment: string): string {
  return segment.split("/").map(encodeURIComponent).join("/");
}

export class FilesClient {
  readonly #baseUrl: string;
  readonly #apiKey: string;
  readonly #fetch: FetchLike;

  constructor(opts: FilesClientOptions) {
    this.#baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.#apiKey = opts.apiKey;
    this.#fetch = opts.fetch ?? globalThis.fetch;
  }

  #url(path: string): string {
    return `${this.#baseUrl}${path}`;
  }

  #headers(extra?: Record<string, string>): Record<string, string> {
    return { authorization: `Bearer ${this.#apiKey}`, ...extra };
  }

  async #fail(res: Response): Promise<FilesError> {
    let code = res.statusText || "error";
    try {
      const j = (await res.json()) as { error?: string };
      if (j && typeof j.error === "string") code = j.error;
    } catch {
      // 본문이 JSON이 아니면 statusText 유지
    }
    return new FilesError(res.status, code);
  }

  /** 객체 업로드. `body`는 문자열/바이트/ReadableStream(스트리밍). 201 시 메타 반환. */
  async putFile(
    bucket: string,
    key: string,
    body: BodyInit,
    opts?: { contentType?: string },
  ): Promise<ObjectMeta> {
    const init: RequestInit & { duplex?: "half" } = {
      method: "PUT",
      headers: this.#headers({
        "content-type": opts?.contentType ?? "application/octet-stream",
      }),
      body,
    };
    if (typeof ReadableStream !== "undefined" && body instanceof ReadableStream) {
      init.duplex = "half"; // Node fetch 스트리밍 바디 요구사항
    }
    const res = await this.#fetch(this.#url(`/api/files/${encodePath(bucket)}/${encodePath(key)}`), init);
    if (!res.ok) throw await this.#fail(res);
    return (await res.json()) as ObjectMeta;
  }

  /** 객체 다운로드. 원시 Response 반환(호출자가 본문/스트림 소비). Range 지원. */
  async getFile(bucket: string, key: string, opts?: { range?: string }): Promise<Response> {
    const headers = this.#headers();
    if (opts?.range) headers.range = opts.range;
    const res = await this.#fetch(
      this.#url(`/api/files/${encodePath(bucket)}/${encodePath(key)}`),
      { method: "GET", headers },
    );
    if (!res.ok && res.status !== 206) throw await this.#fail(res);
    return res;
  }

  /** 객체 메타 헤더(본문 없음). */
  async headFile(
    bucket: string,
    key: string,
  ): Promise<{ size: number; etag: string | null; contentType: string | null }> {
    const res = await this.#fetch(
      this.#url(`/api/files/${encodePath(bucket)}/${encodePath(key)}`),
      { method: "HEAD", headers: this.#headers() },
    );
    if (!res.ok) throw await this.#fail(res);
    return {
      size: Number(res.headers.get("content-length") ?? 0),
      etag: res.headers.get("etag"),
      contentType: res.headers.get("content-type"),
    };
  }

  async deleteFile(bucket: string, key: string): Promise<void> {
    const res = await this.#fetch(
      this.#url(`/api/files/${encodePath(bucket)}/${encodePath(key)}`),
      { method: "DELETE", headers: this.#headers() },
    );
    if (!res.ok) throw await this.#fail(res);
  }

  /** 버킷 객체 목록. */
  async list(bucket: string): Promise<ObjectEntry[]> {
    const res = await this.#fetch(this.#url(`/api/files/${encodePath(bucket)}`), {
      method: "GET",
      headers: this.#headers(),
    });
    if (!res.ok) throw await this.#fail(res);
    return (await res.json()) as ObjectEntry[];
  }

  /** 버킷 생성(admin 키 필요). */
  async putBucket(bucket: string, visibility: Visibility): Promise<BucketMeta> {
    const res = await this.#fetch(this.#url(`/api/buckets/${encodePath(bucket)}`), {
      method: "PUT",
      headers: this.#headers({ "content-type": "application/json" }),
      body: JSON.stringify({ visibility }),
    });
    if (!res.ok) throw await this.#fail(res);
    return (await res.json()) as BucketMeta;
  }

  /** 버킷 목록(admin 키 필요). */
  async listBuckets(): Promise<BucketEntry[]> {
    const res = await this.#fetch(this.#url(`/api/buckets`), {
      method: "GET",
      headers: this.#headers(),
    });
    if (!res.ok) throw await this.#fail(res);
    return (await res.json()) as BucketEntry[];
  }
}
