package main

import (
	"encoding/json"
	"fmt"

	. "github.com/Unique-Divine/jiyuu/pbparse/src"

	// sdk "github.com/cosmos/cosmos-sdk/types"

	// The `_ "embed"` import adds access to files embedded in the running Go
	// program (smart contracts).
	_ "embed"
)

//go:embed eris.json
var ERIS_JSON []byte

// ParseErisJson unmarshals the embedded `eris.json` data into a slice of
// JsonRpcReq structs.
//
// Returns:
//   - out: a slice of JsonRpcReq parsed from `eris.json`.
//   - If there is an error during unmarshaling, the program panics with the error
//     message.
func ParseErisJson() []JsonRpcReq {
	var out []JsonRpcReq
	if err := json.Unmarshal(ERIS_JSON, &out); err != nil {
		panic(fmt.Errorf("error unpacking eris.json: %w", err))
	}
	return out
}

const ABCI_QUERY_ARG = "12df020a86020a83020a242f636f736d7761736d2e7761736d2e76312e4d736745786563757465436f6e747261637412da010a2b6e69626931616838677172746a6c6c6863356c64347278676c3475676c76776c3933616730736836653676123f6e69626931756471717833306377386e776a78746c346c3238796d39686872703933337a6c7138647178666a7a636468766c387932347a6371707a6d68386d1a137b2271756575655f756e626f6e64223a7b7d7d2a550a4a74662f6e69626931756471717833306377386e776a78746c346c3238796d39686872703933337a6c7138647178666a7a636468766c387932347a6371707a6d68386d2f616d704e49424912073130303030303012520a4e0a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a2102a1971ad9cab27ff210a8d9eeddaddc5b1ae1bfa14ccff8e1407566c936a5f4ea12020a00180612001a00"

type AbciQueryParams struct {
	Path string `json:"path"`
	Data string `json:"data"`
}

var ABCI_QUERY_2 = AbciQueryParams{
	Path: "/cosmos.auth.v1beta1.Query/Account",
	Data: "0a2b6e69626931616838677172746a6c6c6863356c64347278676c3475676c76776c3933616730736836653676",
}

const ABCI_QUERY_DATA2 = "0a2b6e69626931616838677172746a6c6c6863356c64347278676c3475676c76776c3933616730736836653676"

func main() {
	jsonRpcReqs := ParseErisJson()
	outReqs := []JsonRpcReqPretty{}
	for idx, jsonRpcReq := range jsonRpcReqs {
		jsonBz, _ := jsonRpcReq.ToJson()
		prettyData, err := PrettyJSONObject(string(jsonBz))
		if err != nil {
			panic(err)
		}
		params := JsonRpcReqParamsPretty{
			Path:  jsonRpcReq.Params.Path,
			Data:  prettyData,
			Prove: jsonRpcReq.Params.Prove,
		}
		outReqs = append(outReqs, JsonRpcReqPretty{
			Jsonrpc: jsonRpcReq.Jsonrpc,
			Id:      jsonRpcReq.Id,
			Method:  jsonRpcReq.Method,
			Params:  params,
		})
		jsonRpcReqs[idx] = jsonRpcReq
	}

	outBz, _ := json.MarshalIndent(outReqs, "", "  ")
	// outBz, _ := json.MarshalIndent(jsonRpcReqs, "", "  ")
	fmt.Printf("%s\n", outBz)

	// const TX_BZ = "CogCCoUCCiQvY29zbXdhc20ud2FzbS52MS5Nc2dFeGVjdXRlQ29udHJhY3QS3AEKK25pYmkxYWg4Z3FydGpsbGhjNWxkNHJ4Z2w0dWdsdndsOTNhZzBzaDZlNnYSP25pYmkxdWRxcXgzMGN3OG53anh0bDRsMjh5bTloaHJwOTMzemxxOGRxeGZqemNkaHZsOHkyNHpjcXB6bWg4bRoTeyJxdWV1ZV91bmJvbmQiOnt9fSpXCkp0Zi9uaWJpMXVkcXF4MzBjdzhud2p4dGw0bDI4eW05aGhycDkzM3pscThkcXhmanpjZGh2bDh5MjR6Y3Fwem1oOG0vYW1wTklCSRIJMjAwMDAwMDAwElIKTgpGCh8vY29zbW9zLmNyeXB0by5zZWNwMjU2azEuUHViS2V5EiMKIQKhlxrZyrJ/8hCo2e7drdxbGuG/oUzP+OFAdWbJNqX06hICCgAYBhIAGgA="
	// {
	// 	out, err := DecodePbMsgFromBase64("/cosmos.tx.v1beta1.Tx", TX_BZ)
	// 	fmt.Printf("out: %s\n", out)
	// 	fmt.Printf("err: %v\n", err)
	// }
}
