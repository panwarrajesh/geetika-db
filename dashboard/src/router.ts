import { useEffect, useState } from 'react';

export type Route =
  | { name: 'orgs' }
  | { name: 'org'; org: string }
  | { name: 'project'; org: string; pid: string }
  | { name: 'env'; org: string; pid: string; env: string }
  | { name: 'docs' }
  | { name: 'ops' }
  | { name: 'settings' }
  | { name: 'notfound' };

export function parseRoute(hash: string): Route {
  const p = hash.replace(/^#/, '').split('/').filter(Boolean).map(decodeURIComponent);
  if (p.length === 0 || (p.length === 1 && p[0] === 'orgs')) return { name: 'orgs' };
  if (p[0] === 'docs' && p.length === 1) return { name: 'docs' };
  if (p[0] === 'ops' && p.length === 1) return { name: 'ops' };
  if (p[0] === 'settings' && p.length === 1) return { name: 'settings' };
  if (p[0] === 'org' && p.length === 2) return { name: 'org', org: p[1] };
  if (p[0] === 'org' && p[2] === 'project' && p.length === 4) return { name: 'project', org: p[1], pid: p[3] };
  if (p[0] === 'org' && p[2] === 'project' && p[4] === 'env' && p.length === 6) {
    return { name: 'env', org: p[1], pid: p[3], env: p[5] };
  }
  return { name: 'notfound' };
}

export function link(...parts: string[]): string {
  return '#/' + parts.map(encodeURIComponent).join('/');
}

export function navigate(href: string): void {
  window.location.hash = href.replace(/^#/, '');
}

export function useRoute(): Route {
  const [hash, setHash] = useState(window.location.hash);
  useEffect(() => {
    const on = () => setHash(window.location.hash);
    window.addEventListener('hashchange', on);
    return () => window.removeEventListener('hashchange', on);
  }, []);
  return parseRoute(hash);
}
