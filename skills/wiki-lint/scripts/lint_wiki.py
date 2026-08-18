#!/usr/bin/env python3
"""Report deterministic structural findings for a WikiOps Wiki."""

import argparse
import html
import json
import re
import uuid
from collections import defaultdict
from pathlib import Path


REQUIRED_FILES = ("index.md", "log.md", "schema.md")
REQUIRED_DIRECTORIES = ("sources", "wiki", "wiki/source-records")
TEXT_EXTENSIONS = {
    ".c",
    ".cc",
    ".conf",
    ".cpp",
    ".css",
    ".csv",
    ".go",
    ".h",
    ".hpp",
    ".htm",
    ".html",
    ".ini",
    ".java",
    ".js",
    ".json",
    ".jsx",
    ".log",
    ".md",
    ".mdx",
    ".py",
    ".rb",
    ".rs",
    ".rst",
    ".sh",
    ".sql",
    ".toml",
    ".ts",
    ".tsx",
    ".txt",
    ".xml",
    ".yaml",
    ".yml",
}
FRONTMATTER_FIELD = re.compile(r"^([A-Za-z_][A-Za-z0-9_-]*):\s*(.*?)\s*$")
HEADING = re.compile(r"^(#{1,6})\s+(.+?)\s*#*\s*$")
EVIDENCE_FIELD = re.compile(r"^\s*[-*]\s+(Source|Lines):\s*(.*?)\s*$")
LINE_RANGE = re.compile(r"^(\d+)-(\d+)$")
PAGE_TYPE_DECLARATION = re.compile(r"^- `([^`]+)`: \S.*$")
WIKILINK = re.compile(r"!?\[\[([^\]\n]+)\]\]")
DEFAULT_EVIDENCE_LAYOUTS = {("evidence", 3, frozenset())}


def parse_args():
    parser = argparse.ArgumentParser(
        description="Report structural findings without changing a WikiOps Wiki."
    )
    parser.add_argument(
        "--wiki-root",
        required=True,
        type=Path,
        help="root of a WikiOps Wiki",
    )
    return parser, parser.parse_args()


def missing_skeleton_findings(wiki_root):
    findings = []
    for relative_path in REQUIRED_FILES:
        if not (wiki_root / relative_path).is_file():
            findings.append(
                {
                    "code": "missing_skeleton_element",
                    "message": f"Required file is missing: {relative_path}",
                    "path": relative_path,
                }
            )
    for relative_path in REQUIRED_DIRECTORIES:
        if not (wiki_root / relative_path).is_dir():
            findings.append(
                {
                    "code": "missing_skeleton_element",
                    "message": f"Required directory is missing: {relative_path}",
                    "path": relative_path,
                }
            )
    return findings


def relative_path(path, wiki_root):
    return path.relative_to(wiki_root).as_posix()


def make_finding(code, path, message, line=None):
    finding = {"code": code, "message": message, "path": path}
    if line is not None:
        finding["line"] = line
    return finding


def scalar_value(value):
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        return value[1:-1]
    if " #" in value:
        return value.split(" #", 1)[0].rstrip()
    return value


def read_markdown(path):
    return path.read_bytes().decode("utf-8", errors="replace").splitlines()


def parse_frontmatter(lines):
    if not lines or lines[0].strip() != "---":
        return None

    fields = {}
    for line in lines[1:]:
        if line.strip() == "---":
            return fields
        match = FRONTMATTER_FIELD.match(line)
        if match:
            fields[match.group(1)] = scalar_value(match.group(2))
    return None


def markdown_line_tokens(lines):
    fence = None
    for line_number, line in enumerate(lines, start=1):
        if fence is None:
            fence_match = re.match(r"(`{3,}|~{3,})", line.lstrip())
            if fence_match:
                marker = fence_match.group(1)
                fence = (marker[0], len(marker))
                yield "fence_start", line_number, line
                continue
            yield "content", line_number, line
            continue

        fence_character, minimum_length = fence
        closing_fence = re.match(
            rf"{re.escape(fence_character)}{{{minimum_length},}}\s*$",
            line.lstrip(),
        )
        if closing_fence:
            fence = None
            yield "fence_end", line_number, line
        else:
            yield "fenced", line_number, line


def markdown_content_lines(lines):
    for kind, line_number, line in markdown_line_tokens(lines):
        if kind == "content":
            yield line_number, line


def markdown_structure(lines, evidence_layouts=None):
    headings = []
    evidence_entries = []
    evidence_layouts = evidence_layouts or DEFAULT_EVIDENCE_LAYOUTS
    heading_stack = {}
    current_heading = None
    current_evidence = None
    for line_number, line in markdown_content_lines(lines):
        heading_match = HEADING.match(line)
        if heading_match:
            level = len(heading_match.group(1))
            name = heading_match.group(2).strip()
            headings.append((level, name, line_number))
            heading_stack = {
                heading_level: heading
                for heading_level, heading in heading_stack.items()
                if heading_level < level
            }
            ancestor_keys = {
                heading_key(heading) for heading in heading_stack.values()
            }
            heading_stack[level] = name
            current_heading = None
            current_evidence = None
            if level >= 2:
                current_heading = {
                    "heading": name,
                    "line": line_number,
                    "fields": {},
                    "field_lines": {},
                }
            if current_heading is not None and any(
                level == entry_level
                and (container is None or container in ancestor_keys)
                and heading_key(name) not in excluded_headings
                for container, entry_level, excluded_headings in evidence_layouts
            ):
                current_evidence = current_heading
                evidence_entries.append(current_evidence)
            continue

        field_match = EVIDENCE_FIELD.match(line)
        if field_match and current_evidence is not None:
            field_name = field_match.group(1).lower()
            current_evidence["fields"][field_name] = scalar_value(
                field_match.group(2)
            ).strip("`")
            current_evidence["field_lines"][field_name] = line_number

    return headings, evidence_entries


def wikilinks(lines):
    links = []
    for line_number, line in markdown_content_lines(lines):
        line = re.sub(r"`+[^`]*?`+", "", line)
        for match in WIKILINK.finditer(line):
            parts = match.group(1).split("|", 1)
            destination = parts[0].strip()
            target, separator, anchor = destination.partition("#")
            links.append(
                {
                    "target": target.strip(),
                    "anchor": anchor.strip() if separator else "",
                    "display": parts[1].strip() if len(parts) == 2 else "",
                    "line": line_number,
                }
            )
    return links


def heading_key(value):
    value = html.unescape(value)
    value = re.sub(r"[`*_~]", "", value)
    return " ".join(value.split()).casefold()


def is_uuid_v4(value):
    try:
        parsed = uuid.UUID(value)
    except (ValueError, AttributeError, TypeError):
        return False
    return parsed.version == 4 and str(parsed) == value


def bundle_file(bundle, locator):
    locator_path = Path(locator)
    if not locator or locator_path.is_absolute():
        return None
    try:
        candidate = (bundle / locator_path).resolve()
        candidate.relative_to(bundle.resolve())
    except (OSError, ValueError):
        return None
    if not candidate.is_file():
        return None
    return candidate


def is_text_file(path):
    if path.suffix.lower() in TEXT_EXTENSIONS:
        return True
    contents = path.read_bytes()
    if b"\x00" in contents:
        return False
    try:
        contents.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return True


def line_count(path):
    contents = path.read_bytes()
    if not contents:
        return 0
    return contents.count(b"\n") + (not contents.endswith(b"\n"))


def schema_page_type_declarations(wiki_root):
    schema_path = wiki_root / "schema.md"
    if not schema_path.is_file():
        return set()

    page_types = set()
    in_page_types = False
    for _, line in markdown_content_lines(read_markdown(schema_path)):
        heading_match = HEADING.match(line)
        if heading_match:
            level = len(heading_match.group(1))
            if level <= 2:
                in_page_types = heading_key(heading_match.group(2)) == "page types"
            continue
        if not in_page_types:
            continue
        declaration_match = PAGE_TYPE_DECLARATION.match(line)
        if declaration_match:
            page_types.add(declaration_match.group(1).strip())
    return page_types


def schema_markdown_blocks(wiki_root):
    schema_path = wiki_root / "schema.md"
    if not schema_path.is_file():
        return []

    blocks = []
    block = None
    for kind, _, line in markdown_line_tokens(read_markdown(schema_path)):
        if kind == "fence_start":
            block = []
        elif kind == "fenced" and block is not None:
            block.append(line)
        elif kind == "fence_end" and block is not None:
            blocks.append(block)
            block = None
    return blocks


def source_record_evidence_layouts(template_lines):
    headings = []
    layouts = set()
    source_lines = []
    for line_number, line in enumerate(template_lines, start=1):
        heading_match = HEADING.match(line)
        if heading_match:
            headings.append(
                (
                    line_number,
                    len(heading_match.group(1)),
                    heading_match.group(2).strip(),
                )
            )
            continue
        field_match = EVIDENCE_FIELD.match(line)
        if field_match and field_match.group(1) == "Source":
            source_lines.append(line_number)

    for line_number in source_lines:
        preceding = [heading for heading in headings if heading[0] < line_number]
        if not preceding:
            continue
        _, entry_level, _ = preceding[-1]
        parent = next(
            (
                heading
                for heading in reversed(preceding[:-1])
                if 2 <= heading[1] < entry_level
            ),
            None,
        )
        container = heading_key(parent[2]) if parent is not None else None
        excluded_headings = frozenset(
            heading_key(name)
            for _, level, name in headings
            if level == entry_level and "<" not in name
        )
        layouts.add((container, entry_level, excluded_headings))
    return layouts


def schema_evidence_layouts(wiki_root):
    template_candidates = []
    for block in schema_markdown_blocks(wiki_root):
        frontmatter = parse_frontmatter(block)
        if not frontmatter or frontmatter.get("type") != "source-record":
            continue
        if not all(
            frontmatter.get(field, "").startswith("<")
            for field in ("source_id", "source_path")
        ):
            continue
        template_candidates.append(block)

    if len(template_candidates) != 1:
        return DEFAULT_EVIDENCE_LAYOUTS
    return (
        source_record_evidence_layouts(template_candidates[0])
        or DEFAULT_EVIDENCE_LAYOUTS
    )


def vault_files(wiki_root):
    return sorted(
        path
        for path in wiki_root.rglob("*")
        if path.is_file() and ".git" not in path.relative_to(wiki_root).parts
    )


def target_indexes(wiki_root, files):
    markdown_by_stem = defaultdict(list)
    file_by_name = defaultdict(list)
    file_by_path = defaultdict(list)
    for path in files:
        relative = relative_path(path, wiki_root)
        file_by_name[path.name.casefold()].append(path)
        file_by_path[relative.casefold()].append(path)
        if path.suffix.lower() == ".md":
            markdown_by_stem[path.stem.casefold()].append(path)
            file_by_path[relative[:-3].casefold()].append(path)
    return markdown_by_stem, file_by_name, file_by_path


def resolve_wikilink(wiki_root, current_path, target, indexes):
    markdown_by_stem, file_by_name, file_by_path = indexes
    if not target:
        return [current_path]

    normalized = target.replace("\\", "/").lstrip("/")
    if "/" in normalized:
        candidates = list(file_by_path.get(normalized.casefold(), []))
        relative_target = (current_path.parent / normalized).resolve()
        try:
            relative_target = relative_target.relative_to(wiki_root.resolve())
        except ValueError:
            relative_target = None
        if relative_target is not None:
            relative_key = relative_target.as_posix().casefold()
            candidates.extend(file_by_path.get(relative_key, []))
            if not Path(normalized).suffix:
                candidates.extend(file_by_path.get(f"{relative_key}.md", []))
        if not candidates:
            suffix = f"/{normalized.casefold()}"
            for path_key, paths in file_by_path.items():
                if path_key.endswith(suffix):
                    candidates.extend(paths)
        return sorted(set(candidates))

    exact_files = file_by_name.get(normalized.casefold(), [])
    if exact_files:
        return exact_files
    return markdown_by_stem.get(normalized.casefold(), [])


def link_and_retrieval_findings(wiki_root):
    findings = []
    files = vault_files(wiki_root)
    indexes = target_indexes(wiki_root, files)
    markdown_files = [path for path in files if path.suffix.lower() == ".md"]
    headings_by_path = {}
    evidence_headings_by_path = {}
    source_records = set()
    wiki_directory = wiki_root / "wiki"
    records_directory = wiki_root / "wiki" / "source-records"
    evidence_layouts = schema_evidence_layouts(wiki_root)

    for path in markdown_files:
        lines = read_markdown(path)
        headings, evidence_entries = markdown_structure(lines, evidence_layouts)
        headings_by_path[path] = defaultdict(list)
        evidence_headings_by_path[path] = defaultdict(list)
        for _, heading, heading_line in headings:
            headings_by_path[path][heading_key(heading)].append(heading_line)
        for entry in evidence_entries:
            evidence_headings_by_path[path][heading_key(entry["heading"])].append(
                entry["line"]
            )
        frontmatter = parse_frontmatter(lines)
        if records_directory in path.parents or (
            wiki_directory in path.parents
            and frontmatter
            and frontmatter.get("type") == "source-record"
        ):
            source_records.add(path)

    markdown_by_stem = indexes[0]
    for record in sorted(source_records):
        collisions = markdown_by_stem.get(record.stem.casefold(), [])
        if len(collisions) > 1:
            collision_paths = ", ".join(
                relative_path(path, wiki_root) for path in collisions if path != record
            )
            findings.append(
                make_finding(
                    "duplicate_source_record_basename",
                    relative_path(record, wiki_root),
                    f"Source Record basename is not Vault-unique; collides with: {collision_paths}",
                )
            )

    scan_paths = []
    for name in REQUIRED_FILES:
        path = wiki_root / name
        if path.is_file():
            scan_paths.append(path)
    if wiki_directory.is_dir():
        scan_paths.extend(sorted(wiki_directory.rglob("*.md")))

    resolved_links_by_path = defaultdict(list)
    for path in sorted(set(scan_paths)):
        path_links = wikilinks(read_markdown(path))
        for link in path_links:
            path_name = relative_path(path, wiki_root)
            targets = resolve_wikilink(wiki_root, path, link["target"], indexes)
            display_target = link["target"] or "this page"
            if not targets:
                findings.append(
                    make_finding(
                        "broken_wikilink",
                        path_name,
                        f"Wikilink target does not exist: {display_target}",
                        link["line"],
                    )
                )
                continue
            if len(targets) > 1:
                findings.append(
                    make_finding(
                        "ambiguous_wikilink",
                        path_name,
                        f"Wikilink target resolves to multiple files: {display_target}",
                        link["line"],
                    )
                )
                continue
            target = targets[0]
            if target in source_records and "/" in link["target"].replace("\\", "/"):
                findings.append(
                    make_finding(
                        "pathful_source_record_link",
                        path_name,
                        "Source Record references must use a pathless Wikilink target.",
                        link["line"],
                    )
                )
            if not link["anchor"]:
                resolved_links_by_path[path].append(target)
                continue

            heading_index = (
                evidence_headings_by_path if target in source_records else headings_by_path
            )
            matching_headings = heading_index.get(target, {}).get(
                heading_key(link["anchor"]), []
            )
            if len(matching_headings) == 1:
                resolved_links_by_path[path].append(target)
                continue
            if target in source_records:
                code = "broken_evidence_anchor"
                message = (
                    "Source Record Evidence anchor does not resolve uniquely: "
                    f"{link['anchor']}"
                )
            else:
                code = "broken_wikilink_anchor"
                message = f"Wikilink heading does not resolve uniquely: {link['anchor']}"
            findings.append(make_finding(code, path_name, message, link["line"]))

    index_path = wiki_root / "index.md"
    if index_path.is_file() and wiki_directory.is_dir():
        ordinary_pages = {
            page
            for page in wiki_directory.rglob("*.md")
            if page not in source_records
        }
        reachable_pages = set()
        frontier = [index_path]
        while frontier:
            origin = frontier.pop()
            for target in resolved_links_by_path.get(origin, []):
                if target not in ordinary_pages or target in reachable_pages:
                    continue
                reachable_pages.add(target)
                frontier.append(target)

        for page in sorted(ordinary_pages - reachable_pages):
            findings.append(
                make_finding(
                    "unreachable_wiki_page",
                    relative_path(page, wiki_root),
                    "Wiki Page is not reachable from index.md through "
                    "unambiguous Wikilinks on ordinary Wiki Pages.",
                )
            )

    return findings


def page_and_source_findings(wiki_root):
    findings = []
    wiki_directory = wiki_root / "wiki"
    records_directory = wiki_directory / "source-records"
    source_store = wiki_root / "sources"
    defined_page_types = schema_page_type_declarations(wiki_root)
    evidence_layouts = schema_evidence_layouts(wiki_root)

    pages = sorted(wiki_directory.rglob("*.md")) if wiki_directory.is_dir() else []
    record_paths = set(
        records_directory.rglob("*.md") if records_directory.is_dir() else []
    )
    page_data = {}
    for page in pages:
        lines = read_markdown(page)
        frontmatter = parse_frontmatter(lines)
        _, evidence_entries = markdown_structure(lines, evidence_layouts)
        page_data[page] = (frontmatter, evidence_entries)
        page_path = relative_path(page, wiki_root)
        if frontmatter is None:
            findings.append(
                make_finding(
                    "missing_page_frontmatter",
                    page_path,
                    "Wiki Page does not begin with YAML frontmatter.",
                )
            )
        elif not frontmatter.get("type"):
            findings.append(
                make_finding(
                    "missing_page_type",
                    page_path,
                    "Wiki Page frontmatter does not define type.",
                )
            )
        elif (
            defined_page_types
            and frontmatter["type"] not in defined_page_types
        ):
            findings.append(
                make_finding(
                    "undefined_page_type",
                    page_path,
                    f"Wiki Page type is not declared in the Wiki Schema's `Page Types` section: {frontmatter['type']}",
                )
            )
        if frontmatter and frontmatter.get("type") == "source-record":
            record_paths.add(page)
            if records_directory not in page.parents:
                findings.append(
                    make_finding(
                        "misplaced_source_record",
                        page_path,
                        "Source Record is outside wiki/source-records/.",
                    )
                )

    valid_bundles = {}
    if source_store.is_dir():
        for bundle in sorted(source_store.iterdir()):
            if bundle.name == ".gitkeep":
                continue
            bundle_path = relative_path(bundle, wiki_root)
            if not bundle.is_dir() or not is_uuid_v4(bundle.name):
                findings.append(
                    make_finding(
                        "invalid_source_bundle",
                        bundle_path,
                        "Source Bundle name is not a canonical UUID v4.",
                    )
                )
                continue
            valid_bundles[bundle.name] = bundle

    records_by_source_id = defaultdict(list)
    valid_record_source_ids = set()
    for record in sorted(record_paths):
        record_path = relative_path(record, wiki_root)
        frontmatter, evidence_entries = page_data.get(record, (None, []))
        frontmatter = frontmatter or {}

        if frontmatter.get("type") != "source-record":
            findings.append(
                make_finding(
                    "invalid_source_record_type",
                    record_path,
                    "Page in the Source Record area must have type: source-record.",
                )
            )

        source_id = frontmatter.get("source_id", "")
        source_id_valid = is_uuid_v4(source_id)
        if not source_id_valid:
            findings.append(
                make_finding(
                    "invalid_source_id",
                    record_path,
                    "Source Record source_id is not a canonical UUID v4.",
                )
            )
        else:
            records_by_source_id[source_id].append(record)
            valid_record_source_ids.add(source_id)
            bundle = valid_bundles.get(source_id)
            evidence_headings = defaultdict(list)
            for entry in evidence_entries:
                evidence_headings[heading_key(entry["heading"])].append(entry)
                if not entry["fields"].get("source", ""):
                    findings.append(
                        make_finding(
                            "missing_evidence_source",
                            record_path,
                            f"Evidence entry has no Source locator: {entry['heading']}",
                            entry["line"],
                        )
                    )
            for entries in evidence_headings.values():
                heading_lines = [entry["line"] for entry in entries]
                if len(heading_lines) > 1:
                    findings.append(
                        make_finding(
                            "duplicate_evidence_anchor",
                            record_path,
                            f"Evidence heading is not unique: {entries[0]['heading']}",
                            heading_lines[1],
                        )
                    )

            if bundle is None:
                findings.append(
                    make_finding(
                        "missing_source_bundle",
                        record_path,
                        f"No valid Source Bundle exists for Source ID {source_id}.",
                    )
                )
            else:
                primary_document = frontmatter.get("source_path", "")
                if bundle_file(bundle, primary_document) is None:
                    findings.append(
                        make_finding(
                            "broken_primary_document",
                            record_path,
                            f"Primary Document does not resolve inside its Source Bundle: {primary_document or '<missing>'}",
                        )
                    )

                for entry in evidence_entries:
                    source_locator = entry["fields"].get("source", "")
                    if not source_locator:
                        continue

                    located_file = bundle_file(bundle, source_locator)
                    if located_file is None:
                        findings.append(
                            make_finding(
                                "broken_evidence_source",
                                record_path,
                                f"Evidence Source does not resolve inside its Source Bundle: {source_locator}",
                                entry["field_lines"]["source"],
                            )
                        )
                        continue

                    lines = entry["fields"].get("lines", "")
                    if is_text_file(located_file):
                        range_match = LINE_RANGE.match(lines)
                        if range_match is None:
                            findings.append(
                                make_finding(
                                    "invalid_evidence_lines",
                                    record_path,
                                    f"Text Evidence needs a start-end line range: {entry['heading']}",
                                    entry["field_lines"].get("lines", entry["line"]),
                                )
                            )
                            continue
                        start, end = map(int, range_match.groups())
                        if start < 1 or end < start or end > line_count(located_file):
                            findings.append(
                                make_finding(
                                    "invalid_evidence_lines",
                                    record_path,
                                    f"Evidence line range is outside {source_locator}: {lines}",
                                    entry["field_lines"]["lines"],
                                )
                            )
                    elif lines:
                        findings.append(
                            make_finding(
                                "unexpected_evidence_lines",
                                record_path,
                                f"Non-text Evidence must omit Lines: {entry['heading']}",
                                entry["field_lines"]["lines"],
                            )
                        )

    for source_id, records in records_by_source_id.items():
        if len(records) > 1:
            for record in records:
                findings.append(
                    make_finding(
                        "duplicate_source_id",
                        relative_path(record, wiki_root),
                        f"Source ID appears in {len(records)} Source Records: {source_id}",
                    )
                )

    for source_id, bundle in valid_bundles.items():
        if source_id not in valid_record_source_ids:
            findings.append(
                make_finding(
                    "unintegrated_source",
                    relative_path(bundle, wiki_root),
                    f"Source Bundle has no Source Record: {source_id}",
                )
            )

    return findings


def main():
    parser, args = parse_args()
    wiki_root = args.wiki_root.absolute()
    if not wiki_root.is_dir():
        parser.error(f"Wiki root does not exist or is not a directory: {wiki_root}")

    findings = missing_skeleton_findings(wiki_root)
    findings.extend(page_and_source_findings(wiki_root))
    findings.extend(link_and_retrieval_findings(wiki_root))
    findings.sort(
        key=lambda finding: (
            finding["code"],
            finding["path"],
            finding.get("line", 0),
            finding["message"],
        )
    )
    print(json.dumps({"findings": findings}, sort_keys=True))


if __name__ == "__main__":
    main()
