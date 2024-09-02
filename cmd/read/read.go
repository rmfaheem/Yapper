package read

import (
	"github.com/spf13/cobra"
)

// readCmd represents the read command
var ReadCmd = &cobra.Command{
	Use:   "read",
	Short: "Read from the database",
	Long: `Read data from the database. Two modes are available:
	1. Single: For a single read request
	2. Flood: For multiple read requests`,
	Run: func(cmd *cobra.Command, args []string) {
		cmd.Help()
	},
}

func init() {
	// rootCmd.AddCommand(readCmd)

	ReadCmd.AddCommand(singleCmd)
	ReadCmd.AddCommand(floodCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// readCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// readCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
