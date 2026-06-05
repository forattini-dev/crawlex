#!/usr/bin/env python3
import base64
import hashlib
import html
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

import requests
import urllib3
from bs4 import BeautifulSoup

ROOT = Path('/home/cyber/Work/FF/crawlex')
sys.path.insert(0, str(ROOT))

from crawlex.access_profiles import (  # noqa: E402
    CircuitBreakerOpen,
    RequestProfileClient,
    classify_response,
    load_profiles,
    profiles_in_pool,
    redact_secret,
)

DATA = ROOT / 'data'
CURATED_DB = DATA / 'medicamentos-brasil-curated.rdb'
CRAWLEX_BIN = '/opt/cargo-target/debug/red'
RAW_STORAGE = str(DATA / 'medicamentos-crawl-raw.rdb')
REDDB_HTTP = 'http://127.0.0.1:8091/query'
REDDB_BASE = REDDB_HTTP.rsplit('/query', 1)[0]
GRAPH_COLLECTION = 'medicamentos_graph'
OUT_JSONL = DATA / 'medicamentos_extracted.jsonl'

HEADERS = {
    'User-Agent': os.environ.get(
        'CRAWLEX_USER_AGENT',
        'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36',
    ),
    'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8',
    'Accept-Language': 'pt-BR,pt;q=0.9,en;q=0.5',
    'Cache-Control': 'no-cache',
    'Pragma': 'no-cache',
}


def redact(value) -> str:
    return re.sub(r'(socks5h?://)[^\\s/@:]+:[^\\s/@]+@', r'\\1[REDACTED]@', str(value), flags=re.I)


def load_env(path: Path = ROOT / '.env'):
    if not path.exists():
        return
    for raw_line in path.read_text().splitlines():
        line = raw_line.strip()
        if not line or line.startswith('#') or '=' not in line:
            continue
        key, value = line.split('=', 1)
        os.environ.setdefault(key.strip(), value.strip().strip('"').strip("'"))


def build_request_proxies():
    load_env()
    enabled = os.environ.get('CRAWLEX_PROXY_ENABLED', 'true').lower() not in {'0', 'false', 'no', 'off'}
    if not enabled:
        return None
    host = os.environ.get('CRAWLEX_PROXY_HOST')
    port = os.environ.get('CRAWLEX_PROXY_PORT')
    user = os.environ.get('CRAWLEX_PROXY_USER')
    password = os.environ.get('CRAWLEX_PROXY_PASSWORD')
    if not all([host, port, user, password]):
        return None
    assert host is not None and port is not None and user is not None and password is not None
    scheme = os.environ.get('CRAWLEX_PROXY_SCHEME', 'socks5h')
    proxy_url = (
        f"{scheme}://"
        f"{urllib.parse.quote(user, safe='')}:{urllib.parse.quote(password, safe='')}"
        f"@{host}:{port}"
    )
    return {'http': proxy_url, 'https': proxy_url}


REQUEST_PROXIES = build_request_proxies()
if REQUEST_PROXIES:
    print(
        'proxy-enabled',
        os.environ.get('CRAWLEX_PROXY_SCHEME', 'socks5h'),
        os.environ.get('CRAWLEX_PROXY_HOST'),
        os.environ.get('CRAWLEX_PROXY_PORT'),
    )

ACCESS_PROFILE_CONFIG = ROOT / 'config' / 'access_profiles.json'
ACCESS_CLIENTS = {}
ACCESS_PROFILE_PRINTED = set()
LOADED_ACCESS_PROFILES = None


def get_loaded_access_profiles():
    global LOADED_ACCESS_PROFILES
    if LOADED_ACCESS_PROFILES is None:
        if not ACCESS_PROFILE_CONFIG.exists():
            LOADED_ACCESS_PROFILES = {}
        else:
            load_env()
            LOADED_ACCESS_PROFILES = load_profiles(ACCESS_PROFILE_CONFIG)
    return LOADED_ACCESS_PROFILES


def get_access_client(profile_name: str):
    if not ACCESS_PROFILE_CONFIG.exists():
        return None
    if profile_name not in ACCESS_CLIENTS:
        profiles = get_loaded_access_profiles()
        if profile_name not in profiles:
            raise RuntimeError(f'unknown_access_profile {profile_name}; config={ACCESS_PROFILE_CONFIG}')
        ACCESS_CLIENTS[profile_name] = RequestProfileClient(profiles[profile_name])
    if profile_name not in ACCESS_PROFILE_PRINTED:
        print('access-profile-enabled', profile_name)
        ACCESS_PROFILE_PRINTED.add(profile_name)
    return ACCESS_CLIENTS[profile_name]


def profile_for_endpoint(endpoint_type: str):
    names = profiles_for_endpoint(endpoint_type)
    return names[0] if names else None


def profiles_for_endpoint(endpoint_type: str):
    if os.environ.get('CRAWLEX_ACCESS_PROFILES_ENABLED', 'true').lower() in {'0', 'false', 'no', 'off'}:
        return []
    if endpoint_type == 'vtex_api':
        explicit = os.environ.get('CRAWLEX_VTEX_ACCESS_PROFILES')
        if explicit:
            return [name.strip() for name in explicit.split(',') if name.strip()]
        legacy = os.environ.get('CRAWLEX_VTEX_ACCESS_PROFILE')
        if legacy:
            return [name.strip() for name in legacy.split(',') if name.strip()]
        pool_name = os.environ.get('CRAWLEX_VTEX_IDENTITY_POOL', 'dsp-vtex-efficient')
        roles_raw = os.environ.get('CRAWLEX_VTEX_IDENTITY_ROLES', 'primary')
        roles = tuple(name.strip() for name in roles_raw.split(',') if name.strip()) or ('primary',)
        profiles = get_loaded_access_profiles()
        selected = profiles_in_pool(profiles, pool_name, 'vtex_api', roles=roles) if profiles else []
        names = [profile.name for profile in selected]
        if names:
            return names
        return ['dsp-vtex-default-sticky', 'dsp-vtex-mobile-sticky']
    if endpoint_type == 'html':
        raw = os.environ.get(
            'CRAWLEX_HTML_ACCESS_PROFILES',
            os.environ.get('CRAWLEX_HTML_ACCESS_PROFILE', 'dsp-html-default-sticky'),
        )
        return [name.strip() for name in raw.split(',') if name.strip()]
    return []

SEEDS = [
    'https://www.drogariasaopaulo.com.br/search?w=dipirona',
    'https://www.drogariasaopaulo.com.br/search?w=ibuprofeno',
    'https://www.drogariasaopaulo.com.br/search?w=losartana',
    'https://www.drogariasaopaulo.com.br/search?w=tadalafila',
    'https://www.drogariasaopaulo.com.br/search?w=rosuvastatina',
    'https://www.drogariasaopaulo.com.br/search?w=glifage',
]

DSP_SEARCH_TERMS = [
    'dipirona', 'ibuprofeno', 'losartana', 'tadalafila', 'rosuvastatina',
    'glifage', 'paracetamol', 'amoxicilina', 'omeprazol', 'loratadina',
    'cetoconazol', 'nimesulida', 'dorflex', 'neosaldina', 'mounjaro',
    'metformina', 'sinvastatina', 'atorvastatina', 'enalapril', 'atenolol',
    'azitromicina', 'prednisona', 'clonazepam', 'sertralina', 'fluoxetina',
    'levotiroxina', 'hidroclorotiazida', 'simeticona', 'pantoprazol', 'desloratadina',
]

PHARMACIES = {
    'consultaremedios.com.br': ('consulta-remedios', 'Consulta Remédios', 'https://consultaremedios.com.br/'),
    'www.drogaraia.com.br': ('drogaraia', 'Droga Raia', 'https://www.drogaraia.com.br/'),
    'www.drogariasaopaulo.com.br': ('drogaria-sao-paulo', 'Drogaria São Paulo', 'https://www.drogariasaopaulo.com.br/'),
    'www.drogariapacheco.com.br': ('drogaria-pacheco', 'Drogaria Pacheco', 'https://www.drogariapacheco.com.br/'),
    'www.bifarma.com.br': ('bifarma', 'Bifarma', 'https://www.bifarma.com.br/'),
}


def slugify(s: str) -> str:
    s = html.unescape(s or '').lower()
    s = re.sub(r'[^a-z0-9áéíóúàâêôãõçüñ]+', '-', s, flags=re.I).strip('-')
    return s[:160] or 'unknown'


def norm(s: str) -> str:
    return re.sub(r'\s+', ' ', html.unescape(s or '')).strip()


def ident(prefix: str, text: str) -> str:
    return f'{prefix}_{hashlib.sha1(norm(text).lower().encode()).hexdigest()[:16]}'


def sql_str(s):
    if s is None:
        return 'NULL'
    return "'" + str(s).replace('\\', '\\\\').replace("'", "''") + "'"


def sql_bool(v):
    return 'true' if v else 'false'


def post_sql(sql: str):
    data = json.dumps({'query': sql}).encode()
    req = urllib.request.Request(REDDB_HTTP, data=data, headers={'content-type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            payload = json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        err_body = e.read().decode(errors='replace')
        raise RuntimeError(f'HTTP {e.code} SQL {sql[:240]}: {err_body}') from e
    if not payload.get('ok'):
        raise RuntimeError(payload)
    return payload


def insert(table, row):
    cols = list(row.keys())
    vals = []
    for v in row.values():
        if isinstance(v, bool): vals.append(sql_bool(v))
        elif isinstance(v, (int, float)) and not isinstance(v, bool): vals.append(str(v))
        elif v is None: vals.append('NULL')
        else: vals.append(sql_str(v))
    post_sql(f"INSERT INTO {table} ({', '.join(cols)}) VALUES ({', '.join(vals)})")

INSERTED_KEYS = set()


def insert_once(table, key, row):
    marker = (table, key)
    if marker in INSERTED_KEYS:
        return
    insert(table, row)
    INSERTED_KEYS.add(marker)


def insert_doc(collection, body):
    # Use the native HTTP document endpoint. The SQL path currently routes these
    # INSERT DOCUMENT statements through a SPARQL parser in this build.
    post_json(f'/collections/{collection}/documents', {'body': body})


GRAPH_NODE_IDS = {}


def post_json(path, body):
    data = json.dumps(body, ensure_ascii=False).encode()
    req = urllib.request.Request(
        REDDB_BASE + path,
        data=data,
        headers={'content-type': 'application/json'},
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            payload = json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        err_body = e.read().decode(errors='replace')
        raise RuntimeError(f'HTTP {e.code} {path}: {err_body}') from e
    if not payload.get('ok'):
        raise RuntimeError(payload)
    return payload


def graph_node(label, node_type, properties):
    if label in GRAPH_NODE_IDS:
        return GRAPH_NODE_IDS[label]
    payload = post_json(
        f'/collections/{GRAPH_COLLECTION}/nodes',
        {'label': label, 'node_type': node_type, 'properties': properties},
    )
    node_id = int(payload['id'])
    GRAPH_NODE_IDS[label] = node_id
    return node_id


def graph_edge(kind, from_label, to_label, properties=None, weight=1.0):
    # Current RedDB 1.10 graph HTTP endpoint expects numeric node ids and a built-in
    # edge label. Store the domain-specific relation in properties.relation_type.
    from_id = GRAPH_NODE_IDS[from_label]
    to_id = GRAPH_NODE_IDS[to_label]
    props = dict(properties or {})
    props.setdefault('relation_type', kind)
    return post_json(
        f'/collections/{GRAPH_COLLECTION}/edges',
        {'label': 'RelatedTo', 'from': from_id, 'to': to_id, 'weight': weight, 'properties': props},
    )


def insert_price_ts(med_id, pharmacy_id, centavos, url, collected_at):
    tags = json.dumps({'medicamento_id': med_id, 'farmacia_id': pharmacy_id, 'url': url}, ensure_ascii=False)
    post_sql(f"INSERT INTO preco_medicamento (metric, value, tags) VALUES ('price.centavos', {int(centavos)}, {tags})")


def fetch(url):
    profile_name = profile_for_endpoint('html')
    client = get_access_client(profile_name) if profile_name else None
    if client:
        r = client.request(url, 'html', timeout=35)
    else:
        r = requests.get(url, headers=HEADERS, timeout=35, verify=False, allow_redirects=True, proxies=REQUEST_PROXIES)
    return r.status_code, r.url, r.text


def fetch_json(url, referer='https://www.drogariasaopaulo.com.br/'):
    headers = dict(HEADERS)
    headers['Accept'] = 'application/json,text/plain,*/*'
    headers['Referer'] = referer
    profile_names = profiles_for_endpoint('vtex_api')
    candidates = profile_names or [None]
    default_attempts = '1' if profile_names else '4'
    attempts = max(1, int(os.environ.get('CRAWLEX_DSP_FETCH_ATTEMPTS', default_attempts)))
    last = None
    for profile_name in candidates:
        client = get_access_client(profile_name) if profile_name else None
        for attempt in range(attempts):
            try:
                if client:
                    r = client.request(url, 'vtex_api', timeout=35)
                else:
                    r = requests.get(url, headers=headers, timeout=35, verify=False, allow_redirects=True, proxies=REQUEST_PROXIES)
                text_prefix = r.text[:1200]
                blocked, reason = classify_response(r.status_code, text_prefix, False)
                last = (profile_name or 'raw', r.status_code, reason, text_prefix[:120])
                if r.status_code in (200, 206) and r.text.lstrip().startswith('['):
                    return r.status_code, r.url, r.json(), (profile_name or 'raw-requests')
                if blocked and client:
                    break
            except CircuitBreakerOpen as e:
                last = (profile_name or 'raw', redact_secret(repr(e)))
                break
            except Exception as e:
                last = (profile_name or 'raw', redact_secret(repr(e)))
                if client:
                    break
            if attempt + 1 < attempts:
                time.sleep(0.8 + attempt * 0.7)
    raise RuntimeError(f'fetch_json_failed {url} profiles={profile_names or ["raw"]} last={redact(last)}')


def cents_from_price(value):
    try:
        if value is None:
            return None
        cents = int(round(float(value) * 100))
        if 0 < cents <= 2000000:
            return cents
    except Exception:
        return None
    return None


def category_from_vtex(product):
    cats = product.get('categories') or []
    if cats:
        parts = [p for p in str(cats[0]).split('/') if p]
        if parts:
            return parts[-1]
    return ''


def infer_active_from_name(name):
    name = norm(name)
    # Common Brazilian medicine title pattern: active ingredient + dosage + brand/lab details.
    m = re.match(r'([A-Za-zÀ-ÿ][A-Za-zÀ-ÿ ]{2,60}?)(?:\s+\d|\s+–|\s+-|$)', name)
    if m:
        candidate = norm(m.group(1))
        stop = {'Kit', 'Leve', 'Pague', 'Combo', 'Solucao', 'Solução', 'Sweet', 'Lipo'}
        if candidate.split()[0] not in stop:
            return candidate
    return ''


def records_from_dsp_api(limit=240):
    seen = {}
    for term in DSP_SEARCH_TERMS:
        url = f'https://www.drogariasaopaulo.com.br/api/catalog_system/pub/products/search/{urllib.parse.quote(term)}'
        try:
            status, final_url, products, access_profile = fetch_json(url, referer=f'https://www.drogariasaopaulo.com.br/search?w={urllib.parse.quote(term)}')
        except Exception as e:
            print('dsp-api-error', term, redact(e))
            continue
        print('dsp-api', term, 'status', status, 'products', len(products), 'profile', access_profile)
        for product in products:
            product_id = str(product.get('productId') or product.get('productReference') or product.get('linkText') or '')
            if not product_id or product_id in seen:
                continue
            name = norm(product.get('productName') or product.get('productTitle') or '')
            link = product.get('link') or f"https://www.drogariasaopaulo.com.br/{product.get('linkText','')}/p"
            prices = []
            available = False
            for sku in product.get('items') or []:
                for seller in sku.get('sellers') or []:
                    offer = seller.get('commertialOffer') or {}
                    available = available or bool(offer.get('IsAvailable'))
                    for key in ('Price', 'ListPrice', 'PriceWithoutDiscount'):
                        cents = cents_from_price(offer.get(key))
                        if cents and cents not in prices:
                            prices.append(cents)
            brand = norm(product.get('brand') or '')
            category = category_from_vtex(product)
            rec = {
                'url': link,
                'host': 'www.drogariasaopaulo.com.br',
                'status': status,
                'title': norm(product.get('productTitle') or name),
                'name': name,
                'medicine_id': ident('med', product_id + '|' + name),
                'active_ingredient': infer_active_from_name(name),
                'laboratory': brand,
                'ean': None,
                'category': category,
                'prices_centavos': prices[:8],
                'fields': {
                    'VTEX productId': product_id,
                    'brand': brand,
                    'categoryId': str(product.get('categoryId') or ''),
                    'available': str(available),
                    'search_source': 'drogariasaopaulo_vtex_api',
                    'access_profile': access_profile,
                },
                'breadcrumbs': [category] if category else [],
                'jsonld': [],
                'html_sha256': hashlib.sha256(json.dumps(product, ensure_ascii=False, sort_keys=True).encode()).hexdigest(),
                'html_bytes': len(json.dumps(product, ensure_ascii=False).encode()),
                'collected_at': int(time.time()),
                'source': 'drogariasaopaulo_vtex_api',
                'raw_product': product,
            }
            seen[product_id] = rec
            if len(seen) >= limit:
                return list(seen.values())
        time.sleep(0.35)
    return list(seen.values())


def discover_product_links(limit=90):
    seen = []
    for seed in SEEDS:
        try:
            status, final_url, text = fetch(seed)
        except Exception as e:
            print('discover-error', seed, redact(e))
            continue
        soup = BeautifulSoup(text, 'lxml')
        for a in soup.find_all('a', href=True):
            href = urllib.parse.urljoin(final_url, a['href'].strip())
            if re.search(r'/p($|[?#])', href) and href not in seen:
                seen.append(href)
        if len(seen) >= limit:
            break
    # Prefer medicine-ish names from ConsultaRemedios/DSP, de-dupe query fragments.
    clean=[]; keys=set()
    for u in seen:
        p=urllib.parse.urlsplit(u)
        key=urllib.parse.urlunsplit((p.scheme,p.netloc,p.path,'',''))
        if key not in keys:
            keys.add(key); clean.append(key)
    return clean[:limit]


def extract_table_fields(soup):
    fields = {}
    for tr in soup.find_all('tr'):
        cells = [norm(c.get_text(' ', strip=True)) for c in tr.find_all(['th','td'])]
        if len(cells) >= 2:
            k = cells[0].rstrip(':')
            v = cells[1]
            if k and v and len(k) < 80:
                fields[k] = v
    text = soup.get_text('\n', strip=True)
    for label in ['Fabricante', 'Laboratório', 'Princípio Ativo', 'Registro MS', 'Registro Anvisa', 'Categoria']:
        if label not in fields:
            m = re.search(label + r'\s*:?\s*\n?\s*([^\n]{2,120})', text, re.I)
            if m:
                fields[label] = norm(m.group(1))
    return fields


def extract_prices(text):
    vals=[]
    # VTEX embeds canonical numeric prices like productPriceFrom:"9.89".
    for raw in re.findall(r"product(?:List)?Price(?:From|To)?[\"']?\s*[:=]\s*[\"']([0-9]{1,6}\.[0-9]{2})[\"']", text):
        cents = int(round(float(raw) * 100))
        if 50 <= cents <= 2000000 and cents not in vals:
            vals.append(cents)
    # schema.org/meta itemprop price.
    for raw in re.findall(r"itemprop=[\"']price[\"'][^>]+content=[\"']([0-9]{1,6}\.[0-9]{2})[\"']", text, re.I):
        cents = int(round(float(raw) * 100))
        if 50 <= cents <= 2000000 and cents not in vals:
            vals.append(cents)
    # Human-rendered Brazilian currency.
    for raw in re.findall(r'R\$\s*([0-9]{1,4}(?:\.[0-9]{3})*,[0-9]{2}|[0-9]{1,4},[0-9]{2})', text):
        cents = int(raw.replace('.', '').replace(',', ''))
        if 50 <= cents <= 2000000 and cents not in vals:
            vals.append(cents)
    return vals[:8]


def extract_jsonld(soup):
    docs=[]
    for s in soup.find_all('script', type='application/ld+json'):
        txt = s.string or s.get_text()
        try:
            docs.append(json.loads(txt))
        except Exception:
            pass
    return docs


def product_from_url(url):
    host=urllib.parse.urlparse(url).netloc
    status, final_url, text = fetch(url)
    soup=BeautifulSoup(text, 'lxml')
    title=norm(soup.title.string if soup.title else '')
    meta = {}
    for tag in soup.find_all('meta'):
        key = tag.get('property') or tag.get('name') or tag.get('itemprop')
        val = tag.get('content')
        if key and val:
            meta.setdefault(key, norm(val))
    h1_candidates = [norm(h.get_text(' ', strip=True)) for h in soup.find_all('h1')]
    h1_candidates = [x for x in h1_candidates if x and x.lower() not in {'drogaria são paulo', 'drogaria sao paulo', 'tudo sobre a drogaria são paulo'}]
    name = h1_candidates[0] if h1_candidates else ''
    if not name:
        name = meta.get('og:title') or meta.get('name') or re.sub(r'\s+[-|].*$', '', title).strip()
    m_product_name = re.search(r"productName[\"']?\s*[:=]\s*[\"']([^\"']{2,180})[\"']", text)
    if (not name or name.lower() in {'drogaria são paulo', 'drogaria sao paulo'}) and m_product_name:
        name = norm(m_product_name.group(1))
    if not name:
        name = urllib.parse.unquote(urllib.parse.urlparse(final_url).path.strip('/').split('/')[0]).replace('-', ' ').title()
    fields=extract_table_fields(soup)
    jsonld=extract_jsonld(soup)
    prices=extract_prices(text)
    breadcrumbs=[]
    for doc in jsonld:
        if isinstance(doc, dict) and doc.get('@type') == 'BreadcrumbList':
            for it in doc.get('itemListElement', []):
                if isinstance(it, dict): breadcrumbs.append(norm(it.get('name') or ''))
    # Heuristics
    active = fields.get('Princípio Ativo') or fields.get('Princípio ativo') or ''
    m_brand = re.search(r"productBrandName[\"']?\s*[:=]\s*[\"']([^\"']{2,120})[\"']", text)
    lab = fields.get('Fabricante') or fields.get('Laboratório') or fields.get('Laboratorio') or ''
    if (not lab or lab.lower().startswith(('desconto ', 'pbm'))) and m_brand:
        lab = norm(m_brand.group(1))
    if not lab:
        lab = meta.get('brand') or ''
    ean = meta.get('gtin13') or meta.get('productID') or None
    m_ean = re.search(r'productEans"?\s*:\s*\["?(\d{8,14})', text)
    if m_ean:
        ean = m_ean.group(1)
    categoria = breadcrumbs[-2] if len(breadcrumbs) >= 2 else (fields.get('Categoria') or '')
    med_id=ident('med', name + '|' + final_url)
    pharmacy_id, pharmacy_name, pharmacy_url = PHARMACIES.get(host, (slugify(host), host, f'https://{host}/'))
    collected=int(time.time())
    rec={
        'url': final_url,
        'host': host,
        'status': status,
        'title': title,
        'name': name,
        'medicine_id': med_id,
        'active_ingredient': active,
        'laboratory': lab,
        'ean': ean,
        'category': categoria,
        'prices_centavos': prices,
        'fields': fields,
        'breadcrumbs': breadcrumbs,
        'jsonld': jsonld,
        'html_sha256': hashlib.sha256(text.encode(errors='ignore')).hexdigest(),
        'html_bytes': len(text.encode()),
        'collected_at': collected,
    }
    return rec


def load_record(rec):
    med_id=rec['medicine_id']; collected=rec['collected_at']; host=rec['host']; url=rec['url']
    pharm_id, pharm_name, pharm_url = PHARMACIES.get(host, (slugify(host), host, f'https://{host}/'))
    source_id = ident('src', url)
    med_label = 'med:'+med_id
    pharmacy_label = 'pharmacy:'+pharm_id
    source_label = 'source:'+source_id

    insert('crawl_sources', {'id': source_id, 'host': host, 'url': url, 'title': rec['title'], 'status': rec['status'], 'blocked': False, 'block_reason': None, 'html_path': None, 'storage_path': RAW_STORAGE, 'collected_at': collected})
    insert_once('farmacias', pharm_id, {'id': pharm_id, 'nome': pharm_name, 'host': host, 'url': pharm_url, 'observed_at_ms': collected})
    insert('medicamentos', {'id': med_id, 'nome': rec['name'], 'slug': slugify(rec['name']), 'ean': rec.get('ean'), 'registro_anvisa': rec['fields'].get('Registro Anvisa') or rec['fields'].get('Registro MS'), 'tipo': None, 'fonte_preferida': host, 'url_preferida': url, 'observed_at_ms': collected})
    insert_doc('raw_product_pages', {'url': url, 'host': host, 'title': rec['title'], 'html_sha256': rec['html_sha256'], 'html_bytes': rec['html_bytes'], 'status': rec['status'], 'collected_at': collected})
    insert_doc('extraction_documents', rec)

    graph_node(med_label, 'medicamento', {'id': med_id, 'nome': rec['name'], 'url': url, 'host': host})
    graph_node(pharmacy_label, 'farmacia', {'id': pharm_id, 'nome': pharm_name, 'host': host, 'url': pharm_url})
    graph_node(source_label, 'crawl_source', {'id': source_id, 'url': url, 'status': rec['status'], 'host': host})
    graph_edge('VENDIDO_EM', med_label, pharmacy_label, {'source_url': url, 'collected_at': collected})
    graph_edge('EXTRAIDO_DE', med_label, source_label, {'source_url': url, 'collected_at': collected})

    if rec['active_ingredient']:
        aid=ident('active', rec['active_ingredient'])
        active_label = 'active:'+aid
        insert_once('principios_ativos', aid, {'id': aid, 'nome': rec['active_ingredient'], 'normalized': slugify(rec['active_ingredient']), 'observed_at_ms': collected})
        graph_node(active_label, 'principio_ativo', {'id': aid, 'nome': rec['active_ingredient'], 'normalized': slugify(rec['active_ingredient'])})
        graph_edge('TEM_PRINCIPIO_ATIVO', med_label, active_label, {'source_url': url, 'confidence': 0.95, 'collected_at': collected})
    if rec['laboratory']:
        lid=ident('lab', rec['laboratory'])
        lab_label = 'lab:'+lid
        insert_once('laboratorios', lid, {'id': lid, 'nome': rec['laboratory'], 'normalized': slugify(rec['laboratory']), 'site': None, 'observed_at_ms': collected})
        graph_node(lab_label, 'laboratorio', {'id': lid, 'nome': rec['laboratory'], 'normalized': slugify(rec['laboratory'])})
        graph_edge('FABRICADO_POR', med_label, lab_label, {'source_url': url, 'confidence': 0.85, 'collected_at': collected})
    if rec['category']:
        cid=ident('cat', rec['category'])
        cat_label = 'cat:'+cid
        insert_once('categorias', cid, {'id': cid, 'nome': rec['category'], 'normalized': slugify(rec['category']), 'parent_id': None, 'source_url': url, 'observed_at_ms': collected})
        graph_node(cat_label, 'categoria', {'id': cid, 'nome': rec['category'], 'normalized': slugify(rec['category'])})
        graph_edge('PERTENCE_A_CATEGORIA', med_label, cat_label, {'source_url': url, 'confidence': 0.75, 'collected_at': collected})
    for idx, cents in enumerate(rec['prices_centavos']):
        offer_id=ident('offer', f'{med_id}|{pharm_id}|{cents}|{idx}|{url}')
        offer_label = 'offer:'+offer_id
        insert('ofertas', {'id': offer_id, 'medicamento_id': med_id, 'farmacia_id': pharm_id, 'nome_no_site': rec['name'], 'preco_centavos': cents, 'preco_texto': f'R$ {cents//100},{cents%100:02d}', 'moeda': 'BRL', 'disponivel': True, 'url': url, 'source_host': host, 'collected_at': collected})
        insert_price_ts(med_id, pharm_id, cents, url, collected)
        graph_node(offer_label, 'oferta', {'id': offer_id, 'preco_centavos': cents, 'preco_texto': f'R$ {cents//100},{cents%100:02d}', 'moeda': 'BRL', 'url': url})
        graph_edge('TEM_OFERTA', med_label, offer_label, {'source_url': url, 'preco_centavos': cents, 'collected_at': collected})


def main():
    start=int(time.time())
    vtex_profile = ','.join(profiles_for_endpoint('vtex_api')) or 'raw-requests'
    insert('extraction_runs', {'id': f'run_{start}', 'started_at': start, 'finished_at': None, 'crawlex_bin': CRAWLEX_BIN, 'crawlex_storage_path': RAW_STORAGE, 'curated_db_path': str(CURATED_DB), 'notes': f'Drogaria São Paulo rebuild from VTEX public catalog API via Crawlex access profile {vtex_profile}.'})
    records = records_from_dsp_api(limit=int(os.environ.get('CRAWLEX_DSP_PRODUCT_LIMIT', '240')))
    print('dsp_api_records', len(records))
    OUT_JSONL.parent.mkdir(parents=True, exist_ok=True)
    loaded = []
    with OUT_JSONL.open('w', encoding='utf-8') as out:
        for rec in records:
            try:
                if rec['status'] < 400 and rec['name']:
                    load_record(rec)
                    loaded.append(rec)
                    out.write(json.dumps(rec, ensure_ascii=False) + '\n')
                    print('loaded', len(loaded), rec['host'], rec['name'][:80], rec['prices_centavos'][:3], rec['active_ingredient'][:50], rec['laboratory'][:50])
            except Exception as e:
                print('product-error', rec.get('url'), str(e))
    finish=int(time.time())
    insert('extraction_runs', {'id': f'run_{start}_complete', 'started_at': start, 'finished_at': finish, 'crawlex_bin': CRAWLEX_BIN, 'crawlex_storage_path': RAW_STORAGE, 'curated_db_path': str(CURATED_DB), 'notes': f'Loaded {len(loaded)} Drogaria São Paulo API product records via access profile {vtex_profile}; see medicamentos_extracted.jsonl'})
    print('DONE loaded_records', len(loaded), 'jsonl', OUT_JSONL)

if __name__ == '__main__':
    urllib3.disable_warnings()
    main()
