package src

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"

	"github.com/cosmos/cosmos-sdk/codec"
)

// JsonRpcReq represents a JSON-RPC request. Presumably, it was extracted from
// the network tab of an Web3 application on Nibiru
type JsonRpcReq struct {
	Jsonrpc string           `json:"jsonrpc"`
	Id      uint64           `json:"id"`
	Method  string           `json:"method"`
	Params  JsonRpcReqParams `json:"params"`
}

// JsonRpcReqParams: Params of a JSON-RPC request
type JsonRpcReqParams struct {
	Path  string `json:"path"`
	Data  string `json:"data"`
	Prove bool   `json:"prove"`
}

// JsonRpcReqParams: Params of a JSON-RPC request
type JsonRpcReqParamsPretty struct {
	Path  string         `json:"path"`
	Data  map[string]any `json:"data"`
	Prove bool           `json:"prove"`
}

type JsonRpcReqPretty struct {
	Jsonrpc string                 `json:"jsonrpc"`
	Id      uint64                 `json:"id"`
	Method  string                 `json:"method"`
	Params  JsonRpcReqParamsPretty `json:"params"`
}

func (jp JsonRpcReqParams) ToPretty(data map[string]any) JsonRpcReqParamsPretty {
	return JsonRpcReqParamsPretty{
		Path:  jp.Path,
		Data:  data,
		Prove: jp.Prove,
	}
}

func (j JsonRpcReq) ToJson() ([]byte, error) {
	path := j.Params.Path
	hexData := j.Params.Data
	return PbMsgFromHexToJson(path, hexData)
}

// PbMsgFromHexToJson decodes a protocol buffer message from a hex-encoded string
// based on a given path.
//
// Parameters:
//   - path: the path used to identify the protocol message type.
//   - hexBz: the hex-encoded protocol buffer message.
//
// Returns:
//   - jsonBz: the JSON-encoded version of the decoded protocol buffer message.
//   - err: any error encountered during decoding or encoding.
func PbMsgFromHexToJson(
	path string,
	hexBz string,
) (jsonBz []byte, err error) {
	pbMsg, ok := PB_MSG_MAP[path]
	if !ok {
		return nil, fmt.Errorf("error resolving path \"%s\"", path)
	}

	protoBz, err := hex.DecodeString(string(hexBz))
	if err != nil {
		return nil, fmt.Errorf("error decoding hex input: %w", err)
	}
	return ProtoBzToJson(protoBz, path, pbMsg)
}

// ProtoBzToJson decodes a protocol buffer message into its JSON representation.
//
// Parameters:
//   - protoBz: the byte slice representing the protocol buffer message.
//   - path: the path used to identify the message type.
//   - pbMsg: the corresponding protocol buffer message type to decode.
//
// Returns:
//   - jsonBz: the JSON-encoded version of the protocol buffer message.
//   - err: any error encountered during decoding or encoding.
func ProtoBzToJson(
	protoBz []byte,
	path string,
	pbMsg codec.ProtoMarshaler,
) (jsonBz []byte, err error) {
	if protoParse, ok := CUSTOM_PROTO_PARSERS[path]; ok {
		return protoParse(protoBz)
	}

	err = PROTO_CODEC.Unmarshal(protoBz, pbMsg)
	// err = protoCdc.UnmarshalInterface(dataBz, pbMsg)
	if err != nil {
		return nil, fmt.Errorf("error unmarhsaling proto message: %w", err)
	}

	jsonBz, err = PROTO_CODEC.MarshalJSON(pbMsg)
	if err != nil {
		return nil, fmt.Errorf("error marshaling proto message to JSON: %w", err)
	}
	return jsonBz, err
}

func ProtoBzToJsonNoCustom(
	protoBz []byte,
	pbMsg codec.ProtoMarshaler,
) (jsonBz []byte, err error) {
	err = PROTO_CODEC.Unmarshal(protoBz, pbMsg)
	// err = protoCdc.UnmarshalInterface(dataBz, pbMsg)
	if err != nil {
		return nil, fmt.Errorf("error unmarhsaling proto message: %w", err)
	}

	jsonBz, err = PROTO_CODEC.MarshalJSON(pbMsg)
	if err != nil {
		return nil, fmt.Errorf("error marshaling proto message to JSON: %w", err)
	}
	return jsonBz, err
}

// PbMsgFromBase64ToJson decodes a protocol buffer message from a base64-encoded
// string based on a given path.
//
// Parameters:
//   - path: the path used to identify the protocol message type.
//   - b64: the base64-encoded protocol buffer message.
//
// Returns:
//   - jsonBz: the JSON-encoded version of the protocol buffer message.
//   - err: any error encountered during decoding or encoding.
func PbMsgFromBase64ToJson(
	path string,
	b64 string,
) (jsonBz []byte, err error) {
	pbMsg, ok := PB_MSG_MAP[path]
	if !ok {
		return nil, fmt.Errorf("error resolving path \"%s\"", path)
	}

	protoBz, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return nil, fmt.Errorf("error decoding base64 input: %w", err)
	}
	return ProtoBzToJson(protoBz, path, pbMsg)
}

func PbMsgFromBase64ToJsonNoCustom(
	path string,
	b64 string,
) (jsonBz []byte, err error) {
	pbMsg, ok := PB_MSG_MAP[path]
	if !ok {
		return nil, fmt.Errorf("error resolving path \"%s\"", path)
	}

	protoBz, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return nil, fmt.Errorf("error decoding base64 input: %w", err)
	}
	return ProtoBzToJsonNoCustom(protoBz, pbMsg)
}
