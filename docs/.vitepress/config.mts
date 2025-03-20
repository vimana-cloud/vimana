import { spawnSync } from "node:child_process";
import { defineConfig } from "vitepress";

// Use the repository root (parent folder) as the Markdown source root.
const SRC_DIR = "..";

// Only serve Markdown documents, and treat all `README` files as directory indices.
const MD_FILEXT = ".md";
const README_NAME = "README" + MD_FILEXT;

// Given a directory root path,
// generate a sidebar for the Vitepress default theme.
//
// The following directory structure:
//     .
//     ├── child/
//     │   ├── README.md
//     │   └── foo-bar.md
//     ├── README.md
//     └── included.md
//
// would generate the following sidebar
// (note the top-level README is not included because it's the home page):
//     [
//       {
//         text: 'child',
//         link: '/child',
//         items: [
//           {
//             text: 'foo-bar',
//             link: '/child/foo-bar',
//           },
//         ],
//       },
//       {
//          text: 'included',
//          link: '/included',
//       },
//     ]
function generateSidebar(): any[] {
    // List all the Markdown files in this repo locally, including untracked files,
    // but not including stuff in Git submodules and `.gitignore` files.
    const mdFiles = spawnSync(
        "git",
        ["ls-files", "--cached", "--others", "--exclude-standard", "--full-name", SRC_DIR],
    ).stdout
        .toString()
        .split(/\n/)
        .filter(path => path.endsWith(MD_FILEXT));
    // Ensure that files in the same directory are grouped together.
    mdFiles.sort();
    // Drop the top-level index.
    // The homepage is special and doesn't need to be in the sidebar.
    const [_index, items, _l] = generateSubSidebar("", mdFiles, 0);
    return items;
}

// Iterate over `mdFiles` paths starting at index `i`,
// generating nested sidebars recursively.
// The files must be sorted.
function generateSubSidebar(dirPrefix: string, mdFiles: string[], i: number) {
    // `dirPrefix` represents a directory.
    // Return the sidebar contents for that directory and all its subdirectories.
    const results = [];
    // Also return whether the `dirPrefix` directory contains a readme.
    var index = false;

    while (i < mdFiles.length) {
        const fullPath = mdFiles[i];
        if (fullPath.startsWith(dirPrefix)) {
            // We're still looking at children of `dirPrefix`.
            const afterPrefix = fullPath.substring(dirPrefix.length);
            const nextSlashIndex = afterPrefix.indexOf("/");
            if (nextSlashIndex == -1) {
                // This is a direct child of `dirPrefix`.
                if (afterPrefix == README_NAME) {
                    // Index documentation is handled at the parent level.
                    index = true;
                } else {
                    const pathNoExt = fullPath.substring(0, fullPath.length - MD_FILEXT.length);
                    const nameNoExt = pathNoExt.substring(dirPrefix.length);
                    results.push({ text: nameNoExt, link: "/" + pathNoExt });
                }
                i += 1;
            } else {
                // This is a subdirectory of the previous iteration. Recurse.
                const childDirname = afterPrefix.substring(0, nextSlashIndex);
                const [childIndex, childResults, childI] = generateSubSidebar(
                    dirPrefix + childDirname + "/",
                    mdFiles,
                    i,
                );
                const result = { text: childDirname };
                if (childIndex) {
                    result.link = dirPrefix + childDirname;
                }
                if (childResults.length > 0) {
                    result.items = childResults;
                    result.collapsed = true;
                }
                results.push(result);
                i = childI;
            }
        } else {
            // No more entries under `dirPrefix`.
            // An outer recursion level may proceed to siblings of `dirPrefix`.
            break;
        }
    }
    return [index, results, i];
}

let mermaidCounter = 0;

// Return a rendered Mermaid diagram as an SVG source.
// The SVG is stripped of its theme style and wrapped in a `<div class="mermaid">.
// On error, return a "danger" custom block with the error message.
function renderMermaid(content: string): string {
    // Use the CLI
    // because calling an async function from sync code is even harder!
    const result = spawnSync(
        "mmdc",
        [
            "--input", "-",
            "--output", "-",
            "--outputFormat", "svg",
            "--svgId", `mermaid-${mermaidCounter++}`,
            "--backgroundColor", "transparent",
        ],
        { input: content },
    );

    if (result.status == 0) {
        // Get rid of the generated style and use the global Mermaid style from `mermaid.css`.
        // The global style responds to the light / dark toggle
        // (and Vue gets angry about `<style>` tags in run-time-generated components anyway).
        const svg = result.stdout.toString();
        return `<div class="mermaid">${svg.replace(/<style>.*<\/style>/, "")}</div>`;
    } else {
        return mermaidError(result.stderr.toString());
    }
}

const HTML_ESCAPES = {
    "&": "&amp;",
    "\"": "&quot;",
    "'": "&apos;",
    "<": "&lt;",
    ">": "&gt;"
};

function mermaidError(error: string): string {
    // Escape HTML special characters.
    error = error.replace(/[&"'<>]/g, c => HTML_ESCAPES[c]);
    return [
        "<div class=\"danger custom-block\">",
        "<p class=\"custom-block-title\">🧜‍♀️ Rendering Error</p>",
        `<pre style="overflow-x:scroll">${error}</pre>`,
        "</div>",
    ].join("");
}


// https://vitepress.dev/reference/site-config
export default defineConfig({
    title: "Vimana Monorepo",
    description: "Vimana internal docs",
    srcDir: SRC_DIR,

    // https://vitepress.dev/reference/default-theme-config
    themeConfig: {
        // Nav bar on top.
        nav: [
            //{ text: 'Home', link: '/' },
            //{ text: 'Examples', link: '/markdown-examples' }
        ],
        // Social links in the nav bar.
        socialLinks: [{ icon: "github", link: "https://github.com/vimana-cloud/vimana" }],
        // Sidebar on the left.
        sidebar: generateSidebar(),
        // Outline on the right.
        outline: { level: "deep" },
    },

    // Treat README.md like index.md in every folder.
    rewrites: {
        "README.md": "index.md",
        ":path(.*)/README.md": ":path/index.md",
    },

    // Get rid of the `.html` suffix in URLs.
    cleanUrls: "without-subfolders",

    vite: {
        publicDir: "docs/public",
    },

    head: [
        // Global style for Mermaid diagrams that responds to the light / dark toggle.
        ["link", { rel: "stylesheet", type: "text/css", href: "/mermaid.css" }],
    ],

    markdown: {
        config: (md) => {
            const fence = md.renderer.rules.fence.bind(md.renderer.rules);
            md.renderer.rules.fence = (tokens, index, options, env, slf) => {
                const token = tokens[index];
                // Shiki normally highlights blocks the the `mermaid` tag,
                // but we want to render those blocks, like how GitHub works.
                if (token.info.trim() === "mermaid") {
                    return renderMermaid(token.content);
                }
                // Fall back to normal rendering for all other tags.
                // Support regular mermaid code highlighting as well using tag `mmd`.
                if (token.info.trim() === "mmd") {
                    //tokens[index].info = "mermaid";
                    token.info = "mermaid";
                }
                return fence(tokens, index, options, env, slf);
            };
        },
    },
});
