#!/usr/bin/env python3
# filepath: scripts/deduplicate_changelog.py

import re
from collections import defaultdict

def deduplicate_changelog(input_file, output_file):
    with open(input_file, 'r') as f:
        content = f.read()

    # Split into version sections
    version_pattern = r'(## \[.*?\].*?)(?=## \[|\Z)'
    version_sections = re.findall(version_pattern, content, re.DOTALL)

    # Track commits: {commit_hash: version_index_where_first_seen}
    commit_to_version = {}

    # Process each version section to identify commits
    parsed_versions = []
    for idx, section in enumerate(version_sections):
        lines = section.split('\n')
        version_header = lines[0] if lines else ""
        parsed_versions.append({
            'header': version_header,
            'lines': lines,
            'index': idx
        })

    # First pass: identify where each commit first appears
    for version_idx, version_data in enumerate(parsed_versions):
        for line in version_data['lines']:
            # Match commit lines with hash
            commit_match = re.search(r'\[([a-f0-9]{7,})\]\(https://github\.com', line)
            if commit_match:
                commit_hash = commit_match.group(1)
                # Only track the first occurrence (oldest version)
                if commit_hash not in commit_to_version:
                    commit_to_version[commit_hash] = version_idx

    # Second pass: filter out duplicates from newer versions
    cleaned_versions = []
    for version_idx, version_data in enumerate(parsed_versions):
        cleaned_lines = []

        for line in version_data['lines']:
            # Check if this is a commit line
            commit_match = re.search(r'\[([a-f0-9]{7,})\]\(https://github\.com', line)

            if commit_match:
                commit_hash = commit_match.group(1)
                # Only keep if this is the version where it first appeared
                if commit_to_version.get(commit_hash) == version_idx:
                    cleaned_lines.append(line)
                # else: skip this line (it's a duplicate in a newer version)
            else:
                # Non-commit line, always keep
                cleaned_lines.append(line)

        # Remove empty sections (Features/Bug Fixes with no items)
        cleaned_section = '\n'.join(cleaned_lines)

        # Clean up empty feature/bug sections
        cleaned_section = re.sub(r'\n### 🚀 Features\n+(?=\n### |## |\Z)', '\n', cleaned_section)
        cleaned_section = re.sub(r'\n### 🐛 Bug Fixes\n+(?=\n### |## |\Z)', '\n', cleaned_section)
        cleaned_section = re.sub(r'\n### ⚙️ Miscellaneous Chores\n+(?=\n### |## |\Z)', '\n', cleaned_section)
        cleaned_section = re.sub(r'\n### 📚 Documentation\n+(?=\n### |## |\Z)', '\n', cleaned_section)

        # Remove multiple consecutive empty lines
        cleaned_section = re.sub(r'\n\n\n+', '\n\n', cleaned_section)

        cleaned_versions.append(cleaned_section)

    # Write cleaned content
    with open(output_file, 'w') as f:
        f.write('\n'.join(cleaned_versions))

    # Calculate statistics
    total_commits = len(commit_to_version)
    duplicates_removed = sum(len([l for l in v['lines'] if re.search(r'\[([a-f0-9]{7,})\]\(', l)])
                            for v in parsed_versions) - total_commits

    print(f"✅ Total unique commits: {total_commits}")
    print(f"🗑️  Duplicate entries removed: {duplicates_removed}")
    print(f"📝 Cleaned changelog written to {output_file}")
    print(f"\nSummary by version:")
    for version_data in parsed_versions:
        version_name = re.search(r'## \[(.*?)\]', version_data['header'])
        if version_name:
            commit_count = sum(1 for line in version_data['lines']
                             if re.search(r'\[([a-f0-9]{7,})\]\(', line))
            first_appears_count = sum(1 for commit_hash, ver_idx in commit_to_version.items()
                                    if ver_idx == version_data['index'])
            print(f"  {version_name.group(1)}: {first_appears_count} changes (was {commit_count})")

if __name__ == '__main__':
    deduplicate_changelog('CHANGELOG.md', 'CHANGELOG_CLEAN.md')
