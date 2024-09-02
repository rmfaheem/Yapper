package config

import (
	"bytes"
	"encoding/json"
	"os"
)

type GossipSeed struct {
	Endpoint string `json:"endpoint"`
	Port     string `json:"port"`
}

type Config struct {
	Cluster        bool         `json:"cluster"`
	GossipSeed     []GossipSeed `json:"gossipSeed"`
	Tls            bool         `json:"tls"`
	TlsVerifyCert  bool         `json:"tlsVerifyCert"`
	RootCaPath     string       `json:"rootCaPath"`
	NodePreference string       `json:"nodePreference"`
	Username       string       `json:"username"`
	Password       string       `json:"password"`
}

func (c Config) BuildConnectionString() string {
	var connectionString bytes.Buffer
	connectionString.WriteString("esdb")
	if c.Cluster {
		connectionString.WriteString("+discover")
	}
	connectionString.WriteString("://")
	connectionString.WriteString(c.Username)
	connectionString.WriteString(":")
	connectionString.WriteString(c.Password)
	connectionString.WriteString("@")

	for i, node := range c.GossipSeed {
		if i > 0 {
			connectionString.WriteString(",")
		}
		connectionString.WriteString(node.Endpoint)
		connectionString.WriteString(":")
		connectionString.WriteString(node.Port)
	}

	var optionsBuilder bytes.Buffer

	// tls is true by default, hence we only check if false
	if !c.Tls {
		optionsBuilder.WriteString("tls=false;")
	}
	// if tls is true but verify cert is not
	if c.Tls && !c.TlsVerifyCert {
		optionsBuilder.WriteString("tlsVerifyCert=false;")
	}
	// if tls is true and a root ca path is specified
	if c.Tls && len(c.RootCaPath) != 0 {
		optionsBuilder.WriteString("tlsCaFile=")
		optionsBuilder.WriteString(c.RootCaPath)
		optionsBuilder.WriteString(";")
	}

	if c.NodePreference != "" {
		optionsBuilder.WriteString("nodePreference=")
		optionsBuilder.WriteString(c.NodePreference)
		optionsBuilder.WriteString(";")
	}
	// if options were added, concat the options then return
	if len(optionsBuilder.Bytes()) != 0 {
		connectionString.WriteString("?")
		connectionString.WriteString(optionsBuilder.String())
		return connectionString.String()
	}
	return connectionString.String()
}

func LoadConfigFromFile(path string) *Config {
	b, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}

	var c Config
	if err = json.Unmarshal(b, &c); err != nil {
		panic(err)
	}

	return &c
}
