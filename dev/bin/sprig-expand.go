// Read a Go template from stdin and execute it with JSON data from the command-line argument.
// The result is written to stdout.
// Sprig template functions are available.
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"text/template"

	"github.com/Masterminds/sprig/v3"
)

// In addition to the standard Sprig library, also provide this function called `readFile`,
// which simply returns the contents of a file at the given path as a string.
func readFile(path string) (string, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	return string(b), nil
}

func main() {
	if len(os.Args) > 2 {
		fmt.Fprintf(os.Stderr, "Usage: %s [JSON]\n", os.Args[0])
		os.Exit(1)
	}

	var dot any
	if len(os.Args) == 2 {
		if err := json.Unmarshal([]byte(os.Args[1]), &dot); err != nil {
			fmt.Fprintf(os.Stderr, "Error parsing JSON: %v\n", err)
			os.Exit(1)
		}
	}

	tmplBytes, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error reading stdin: %v\n", err)
		os.Exit(1)
	}

	tmpl, err := template.New("tmpl").
		Funcs(sprig.FuncMap()).
		Funcs(template.FuncMap{"readFile": readFile}).
		Parse(string(tmplBytes))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error parsing template: %v\n", err)
		os.Exit(1)
	}

	if err := tmpl.Execute(os.Stdout, dot); err != nil {
		fmt.Fprintf(os.Stderr, "Error executing template: %v\n", err)
		os.Exit(1)
	}
}
