import { describe, it, expect, vi } from "vitest";
import { FilesClient, FilesError } from "./index";

const meta = {
  contentType: "text/plain",
  size: 5,
  sha256: "abc",
  createdAt: "2026-01-01T00:00:00Z",
  uploadedBy: "page",
};

type FetchArgs = Parameters<typeof fetch>;

function mockFetch(impl: (...args: FetchArgs) => Promise<Response>) {
  return vi.fn(impl);
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function headers(init: RequestInit | undefined): Record<string, string> {
  return init!.headers as Record<string, string>;
}

describe("FilesClient", () => {
  it("putFile sends PUT with auth, content-type and body; returns meta", async () => {
    const fetch = mockFetch(async () => jsonResponse(meta, 201));
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    const got = await c.putFile("skills", "a/b.txt", "hello", {
      contentType: "text/plain",
    });
    expect(got).toEqual(meta);
    const [url, init] = fetch.mock.calls[0]!;
    expect(url).toBe("http://h/api/files/skills/a/b.txt");
    expect(init!.method).toBe("PUT");
    expect(headers(init).authorization).toBe("Bearer K");
    expect(headers(init)["content-type"]).toBe("text/plain");
    expect(init!.body).toBe("hello");
  });

  it("trims trailing slash on baseUrl and url-encodes path segments", async () => {
    const fetch = mockFetch(async () => jsonResponse(meta, 201));
    const c = new FilesClient({ baseUrl: "http://h/", apiKey: "K", fetch });
    await c.putFile("skills", "dir/a b.txt", "x");
    expect(fetch.mock.calls[0]![0]).toBe("http://h/api/files/skills/dir/a%20b.txt");
  });

  it("getFile passes Range header and returns the raw Response", async () => {
    const fetch = mockFetch(
      async () =>
        new Response("hello", {
          status: 206,
          headers: { "content-range": "bytes 0-4/11" },
        }),
    );
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    const res = await c.getFile("skills", "a", { range: "bytes=0-4" });
    expect(await res.text()).toBe("hello");
    const [, init] = fetch.mock.calls[0]!;
    expect(headers(init).range).toBe("bytes=0-4");
  });

  it("headFile parses size/etag/content-type headers", async () => {
    const fetch = mockFetch(
      async () =>
        new Response(null, {
          status: 200,
          headers: {
            "content-length": "42",
            etag: '"abc"',
            "content-type": "text/plain",
          },
        }),
    );
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    const h = await c.headFile("skills", "a");
    expect(h.size).toBe(42);
    expect(h.etag).toBe('"abc"');
    expect(h.contentType).toBe("text/plain");
    expect(fetch.mock.calls[0]![1]!.method).toBe("HEAD");
  });

  it("deleteFile issues DELETE", async () => {
    const fetch = mockFetch(async () => new Response(null, { status: 204 }));
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    await c.deleteFile("skills", "a");
    expect(fetch.mock.calls[0]![1]!.method).toBe("DELETE");
  });

  it("list returns object entries", async () => {
    const entries = [{ key: "a.txt", ...meta }];
    const fetch = mockFetch(async () => jsonResponse(entries));
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    expect(await c.list("skills")).toEqual(entries);
    expect(fetch.mock.calls[0]![0]).toBe("http://h/api/files/skills");
  });

  it("putBucket posts visibility json", async () => {
    const bm = { visibility: "public", owner: "page", createdAt: "t" };
    const fetch = mockFetch(async () => jsonResponse(bm, 201));
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    const got = await c.putBucket("skills", "public");
    expect(got).toEqual(bm);
    const [url, init] = fetch.mock.calls[0]!;
    expect(url).toBe("http://h/api/buckets/skills");
    expect(JSON.parse(init!.body as string)).toEqual({ visibility: "public" });
  });

  it("listBuckets returns bucket entries", async () => {
    const entries = [{ bucket: "skills", visibility: "public", owner: "o", createdAt: "t" }];
    const fetch = mockFetch(async () => jsonResponse(entries));
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    expect(await c.listBuckets()).toEqual(entries);
    expect(fetch.mock.calls[0]![0]).toBe("http://h/api/buckets");
  });

  it("throws FilesError with status and code on non-ok responses", async () => {
    const fetch = mockFetch(async () => jsonResponse({ error: "forbidden" }, 403));
    const c = new FilesClient({ baseUrl: "http://h", apiKey: "K", fetch });
    await expect(c.deleteFile("skills", "x")).rejects.toBeInstanceOf(FilesError);
    await expect(c.list("skills")).rejects.toMatchObject({ status: 403, code: "forbidden" });
  });
});
