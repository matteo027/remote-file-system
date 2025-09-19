import { DataSource } from "typeorm";
import { User } from "./entities/User";
import { File } from "./entities/File";
import { Group } from "./entities/Group";
import { Path } from "./entities/Path";

export const AppDataSource = new DataSource({
  type: "sqlite",
  database: "metadata.sqlite",
  synchronize: true,
  logging: false,
  entities: [User, File, Group, Path],
});
