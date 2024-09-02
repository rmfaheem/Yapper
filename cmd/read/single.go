package read

import (
	"fmt"

	"github.com/spf13/cobra"
)

// readCmd represents the read command
var singleCmd = &cobra.Command{
	Use:   "single",
	Short: "Send a single read request to the database",
	Long: `Send a single read request to the database.
	Example:
	$`,
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println("single read called")
	},
}

func init() {
	// rootCmd.AddCommand(readCmd)

	// Here you will define your flags and configuration settings.

	// Cobra supports Persistent Flags which will work for this command
	// and all subcommands, e.g.:
	// readCmd.PersistentFlags().String("foo", "", "A help for foo")

	// Cobra supports local flags which will only run when this command
	// is called directly, e.g.:
	// readCmd.Flags().BoolP("toggle", "t", false, "Help message for toggle")
}
