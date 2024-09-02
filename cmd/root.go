package cmd

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/rmfaheem/yapper/cmd/read"
	"github.com/rmfaheem/yapper/cmd/subscribe"
	"github.com/rmfaheem/yapper/cmd/write"
	"github.com/rmfaheem/yapper/internal/config"
	"github.com/rmfaheem/yapper/internal/database"
	"github.com/spf13/cobra"
)

var cfgFile string
var conf *config.Config
var DB *database.Database

var rootCmd = &cobra.Command{
	Use:   "yapper",
	Short: "Yapper is test client for EventStoreDB",
	Long: `Yapper is test client for EventStoreDB.
It can read, write and subscribe to the database.`,
	Run: func(cmd *cobra.Command, args []string) {
		cmd.Help()
	},
}

func init() {
	cobra.OnInitialize(initConfig)

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default is $HOME/.yapper.json)")

	rootCmd.AddCommand(configCmd)
	rootCmd.AddCommand(write.WriteCmd)
	rootCmd.AddCommand(read.ReadCmd)
	rootCmd.AddCommand(subscribe.SubscribeCmd)
	rootCmd.AddCommand(tuiCmd)
}

func initConfig() {
	// Find home directory.
	home, err := os.UserHomeDir()
	cobra.CheckErr(err)

	if cfgFile != "" {
		// Use config file from the flag.
		fmt.Println("Using config file:", cfgFile)
		conf = config.LoadConfigFromFile(cfgFile)

	} else if _, err = os.Stat(home + "/.yapper.json"); os.IsNotExist(err) {

		// check if default config file exists
		fmt.Printf("No config file found. Creating default config file at:%s/%s\n", home, ".yapper.json")

		c := config.Config{}
		c.Cluster = false

		gossipSeeds := [...]config.GossipSeed{
			{
				Endpoint: "127.0.0.1",
				Port:     "2113",
			},
		}
		c.GossipSeed = gossipSeeds[:]

		c.Tls = false
		c.TlsVerifyCert = false

		c.Username = "admin"
		c.Password = "changeit"

		c.NodePreference = "random"

		file, err := os.Create(home + "/.yapper.json")
		if err != nil {
			panic(err)
		}
		defer file.Close()
		jsonBytes, err := json.Marshal(c)
		if err != nil {
			panic(err)
		}
		_, err = file.Write(jsonBytes)
		if err != nil {
			panic(err)
		}
		conf = &c

	} else {
		fmt.Println("Using default config file: ", home+"/.yapper.json")
		conf = config.LoadConfigFromFile(home + "/.yapper.json")
	}

	DB = database.Init(conf)
}

func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
}
