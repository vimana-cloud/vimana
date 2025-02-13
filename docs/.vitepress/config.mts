import { execSync } from "node:child_process";
import { defineConfig } from "vitepress";
import { withMermaid } from "vitepress-plugin-mermaid";

// Use the repository root (parent folder) as the Markdown source root.
const SRC_DIR = "..";

// Only serve Markdown documents, and treat all `README` files as directory indices.
const MD_FILEXT = ".md";
const README_NAME = "README" + MD_FILEXT;

// Given a directory root path,
// generate a sidebar for the Vitepress default theme.
//
// The following directory structure:
//     - .
//       - child/
//         - README.md
//         - foo-bar.md
//       - README.md
//       - included.md
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
    const mdFiles = execSync(
        `git ls-files --cached --others --exclude-standard --full-name ${SRC_DIR}`,
    )
        .toString()
        .split(/\n/)
        .filter((path) => path.endsWith(MD_FILEXT));
    // Enforce that files in the same directory are grouped together.
    mdFiles.sort();
    // Drop the top-level index.
    // The homepage is special and doesn't need to be in the sidebar.
    const [_index, items, _l] = generateSubSidebar("", mdFiles, 0);
    return items;
}

// Iterate over `mdFiles` paths starting at index `i`.
// Generating nested sidebars recursively.
function generateSubSidebar(dirPrefix: string, mdFiles: string[], i: number) {
    const results = [];
    var index = false;
    while (i < mdFiles.length) {
        const fullPath = mdFiles[i];
        if (fullPath.startsWith(dirPrefix)) {
            const afterPrefix = fullPath.substring(dirPrefix.length);
            const nextSlashIndex = afterPrefix.indexOf("/");
            if (nextSlashIndex == -1) {
                // This is in the same directory as the previous iteration.
                if (afterPrefix == README_NAME) {
                    // Index documentation is linked at the parent level.
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
            // No more entries at the previous iteration's level. Quit.
            break;
        }
    }
    return [index, results, i];
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
});
