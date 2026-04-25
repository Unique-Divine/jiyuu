package ctxcat

const (
	codeMarkerDepth4 string = "````"
)

var globalIgnoreLines = []string{
	".git",
	".gitmodules",
	".DS_Store",
	".idea",
	".vscode",
	"*.png",
	"*.jpg",
	"*.lock",
	"*.lockb",
	"*lock.json",
}
