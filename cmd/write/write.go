package write

import (
	"github.com/spf13/cobra"
)

// writeCmd represents the write command
var WriteCmd = &cobra.Command{
	Use:   "write",
	Short: "Write to the database",
	Long: `Write randomly generated data to the database.
	Two modes are available:
	1. Single: Send a single write request to the database.
	2. Flood: Send multiple write requests to the database.`,
	Run: func(cmd *cobra.Command, args []string) {
		cmd.Help()
	},
}

func init() {
	// rootCmd.AddCommand(writeCmd)

	WriteCmd.AddCommand(singleCmd)
	WriteCmd.AddCommand(floodCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// writeCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// writeCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
